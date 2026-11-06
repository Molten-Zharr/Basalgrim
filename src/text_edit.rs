//! Grapheme-aware state shared by every single-line editor and renderer.
use crate::fs;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

#[derive(Default, Debug, Clone)]
pub struct TextEdit {
    pub cursor: usize,
    pub anchor: usize,
    pub revision: u64,
}

pub fn boundaries(text: &str) -> Vec<usize> {
    let mut result = vec![0];
    let mut offset = 0;
    for grapheme in
        ratatui::text::Span::raw(text).styled_graphemes(ratatui::style::Style::default())
    {
        offset += grapheme.symbol.len();
        result.push(offset);
    }
    result
}

fn floor(text: &str, position: usize) -> usize {
    boundaries(text)
        .into_iter()
        .take_while(|&i| i <= position)
        .last()
        .unwrap_or(0)
}

pub fn width(text: &str) -> usize {
    fs::clean(text).width()
}

impl TextEdit {
    pub fn reset(&mut self, text: &str, selection: Range<usize>) {
        self.anchor = floor(text, selection.start);
        self.cursor = floor(text, selection.end);
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn selection(&self, text: &str) -> Range<usize> {
        let cursor = floor(text, self.cursor);
        let anchor = floor(text, self.anchor);
        cursor.min(anchor)..cursor.max(anchor)
    }
    pub fn select_all(&mut self, text: &str) {
        self.reset(text, 0..text.len());
    }
    pub fn move_to(&mut self, text: &str, position: usize, extend: bool) {
        self.cursor = floor(text, position);
        if !extend {
            self.anchor = self.cursor;
        }
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn previous(&self, text: &str) -> usize {
        boundaries(text)
            .into_iter()
            .take_while(|&i| i < self.cursor)
            .last()
            .unwrap_or(0)
    }
    pub fn next(&self, text: &str) -> usize {
        boundaries(text)
            .into_iter()
            .find(|&i| i > self.cursor)
            .unwrap_or(text.len())
    }
    pub fn word(&self, text: &str, forward: bool) -> usize {
        let points = boundaries(text);
        let mut i = points
            .iter()
            .position(|&p| p == floor(text, self.cursor))
            .unwrap_or(0);
        let word = |i: usize| {
            text[points[i]..points[i + 1]]
                .chars()
                .any(|c| c.is_alphanumeric() || c == '_')
        };
        if forward {
            while i + 1 < points.len() && word(i) {
                i += 1;
            }
            while i + 1 < points.len() && !word(i) {
                i += 1;
            }
        } else {
            while i > 0 && !word(i - 1) {
                i -= 1;
            }
            while i > 0 && word(i - 1) {
                i -= 1;
            }
        }
        points[i]
    }
    pub fn replace(&mut self, text: &mut String, inserted: &str) {
        let selection = self.selection(text);
        let kept = text[..selection.start].chars().count() + text[selection.end..].chars().count();
        let inserted: String = inserted
            .chars()
            .filter(|c| !c.is_control())
            .take(4096usize.saturating_sub(kept))
            .collect();
        if inserted.is_empty() {
            return;
        }
        text.replace_range(selection.clone(), &inserted);
        self.move_to(text, selection.start + inserted.len(), false);
    }
    pub fn delete(&mut self, text: &mut String, backward: bool, word: bool) {
        let mut selection = self.selection(text);
        if selection.is_empty() {
            let position = if word {
                self.word(text, !backward)
            } else if backward {
                self.previous(text)
            } else {
                self.next(text)
            };
            selection = self.cursor.min(position)..self.cursor.max(position);
        }
        text.replace_range(selection.clone(), "");
        self.move_to(text, selection.start, false);
    }
    pub fn visible_start(&self, text: &str, columns: u16) -> usize {
        let cursor = floor(text, self.cursor);
        let mut start = cursor;
        let mut used = 0;
        for pair in boundaries(text).windows(2).rev().filter(|p| p[1] <= cursor) {
            let w = width(&text[pair[0]..pair[1]]);
            if used + w > usize::from(columns.saturating_sub(1)) {
                break;
            }
            used += w;
            start = pair[0];
        }
        start
    }
    pub fn position(text: &str, start: usize, column: u16) -> usize {
        let first = floor(text, start);
        let mut position = first;
        let mut used = 0;
        for pair in boundaries(text).windows(2).filter(|pair| pair[0] >= first) {
            let w = width(&text[pair[0]..pair[1]]);
            if used + w > usize::from(column) {
                break;
            }
            used += w;
            position = pair[1];
        }
        position
    }
}
