//! Theme-independent syntax spans, computed only by the preview worker.
use crate::operations::Cancellation;
use std::{
    ops::Range,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};
use two_face::re_exports::syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

const MAX_LINE_BYTES: usize = 2048;
const MAX_SPANS: usize = 16_384;
const PARSE_BUDGET: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Comment,
    Keyword,
    String,
    Constant,
    Name,
}

#[derive(Clone, Debug)]
pub struct Token {
    /// Byte offsets into the cleaned, numbered display line, never the raw input.
    pub range: Range<usize>,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct CodePreview {
    pub language: String,
    pub lines: Vec<Vec<Token>>,
    pub limited: bool,
}

fn kind(stack: &ScopeStack) -> Kind {
    static RULES: OnceLock<Vec<(Scope, Kind)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            ("comment", Kind::Comment),
            ("string", Kind::String),
            ("constant", Kind::Constant),
            ("keyword", Kind::Keyword),
            ("storage", Kind::Keyword),
            ("entity.name", Kind::Name),
            ("entity.other.attribute-name", Kind::Name),
            ("support", Kind::Name),
        ]
        .into_iter()
        .map(|(scope, kind)| (Scope::new(scope).expect("Known scope"), kind))
        .collect()
    });
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| {
            rules
                .iter()
                .find(|(prefix, _)| prefix.is_prefix_of(*scope))
                .map(|(_, kind)| *kind)
        })
        .unwrap_or(Kind::Plain)
}

/// Never reopen the path or load user grammars. Both filenames and first lines
/// are hints, not instructions; unknown/plain text receives no syntax styling.
pub fn highlight(path: &Path, lines: &[String], cancel: &Cancellation) -> Option<CodePreview> {
    if cancel.is_cancelled() || lines.is_empty() {
        return None;
    }
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(two_face::syntax::extra_newlines);
    let filename = path.file_name()?.to_str().unwrap_or("");
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let first = lines.first()?.get(5..)?.trim_start_matches('\u{feff}');
    let syntax = syntaxes
        .find_syntax_by_extension(filename)
        .or_else(|| syntaxes.find_syntax_by_extension(&extension))
        .or_else(|| {
            // Avoid running first-line regexes against minified data, and don't
            // reinterpret explicitly named plain text as a script.
            (first.len() <= MAX_LINE_BYTES)
                .then(|| syntaxes.find_syntax_by_first_line(first))
                .flatten()
        })?;
    if std::ptr::eq(syntax, syntaxes.find_syntax_plain_text()) {
        return None;
    }
    let mut result = CodePreview {
        language: syntax.name.clone(),
        lines: Vec::new(),
        limited: false,
    };
    let mut parser = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let deadline = Instant::now() + PARSE_BUDGET;
    let mut spans = 0;
    for line in lines {
        if cancel.is_cancelled() {
            return None;
        }
        let source = line.get(5..)?;
        if source.len() > MAX_LINE_BYTES || Instant::now() >= deadline {
            result.limited = true;
            break;
        }
        // Newlines are significant for multiline strings and comments.
        let source = format!("{source}\n");
        let Ok(ops) = parser.parse_line(&source, syntaxes) else {
            result.limited = true;
            break;
        };
        let mut tokens: Vec<Token> = Vec::new();
        let mut start = 0;
        let end = source.len() - 1;
        let mut valid = true;
        for (at, op) in ops {
            let at = at.min(end);
            if at > start {
                let kind = kind(&stack);
                if let Some(previous) = tokens.last_mut().filter(|t| t.kind == kind) {
                    previous.range.end = at + 5;
                } else {
                    tokens.push(Token {
                        range: start + 5..at + 5,
                        kind,
                    });
                }
            }
            start = at;
            if stack.apply(&op).is_err() {
                valid = false;
                break;
            }
        }
        if start < end {
            tokens.push(Token {
                range: start + 5..end + 5,
                kind: kind(&stack),
            });
        }
        spans += tokens.len();
        if !valid || spans > MAX_SPANS || Instant::now() >= deadline {
            result.limited = true;
            break;
        }
        result.lines.push(tokens);
    }
    Some(result)
}
