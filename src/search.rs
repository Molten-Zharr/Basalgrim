//! Complete name search with a disk-backed directory queue and paged results.
use crate::operations::Cancellation;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
pub const PAGE_SIZE: usize = 128;
#[derive(Clone, Default)]
pub struct Results {
    pub paths: Vec<PathBuf>,
    pub offset: usize,
    pub total: usize,
    pub scanned: usize,
    pub errors: usize,
    pub failure: Option<String>,
    pub done: bool,
    pub stopped: bool,
}
impl Results {
    pub fn path(&self, index: usize) -> Option<&PathBuf> {
        self.paths.get(index.checked_sub(self.offset)?)
    }
    pub fn status(&self) -> &'static str {
        if self.stopped {
            "Остановлен"
        } else if self.failure.is_some() {
            "Прерван"
        } else if !self.done {
            "Поиск…"
        } else if self.errors > 0 {
            "С пропусками"
        } else {
            "Завершён"
        }
    }
}
struct Request {
    id: u64,
    root: PathBuf,
    query: String,
    hidden: bool,
    cancel: Cancellation,
}
enum Command {
    Start(Request),
    Clear,
    Page(u64, usize),
}
type Mailbox = Arc<Mutex<Option<(u64, Results)>>>;
pub struct Search {
    send: mpsc::Sender<Command>,
    output: Mailbox,
    cancel: Cancellation,
    wanted: usize,
    pub id: u64,
    pub results: Results,
}
struct Run {
    db: Connection,
    directory: Option<std::fs::ReadDir>,
    request: Request,
    results: Results,
}
impl Run {
    fn new(request: Request) -> anyhow::Result<Self> {
        // An unnamed SQLite database spills beyond its cache into an unlinked
        // temporary file on Unix, so a killed process cannot leave search caches.
        let db = Connection::open("")?;
        // Reconstructible private search data, not the durable operations journal.
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=OFF; PRAGMA cache_size=-1024; PRAGMA temp_store=FILE;
            CREATE TABLE queue (id INTEGER PRIMARY KEY, path BLOB NOT NULL);
            CREATE TABLE hits (id INTEGER PRIMARY KEY, path BLOB NOT NULL);")?;
        db.execute(
            "INSERT INTO queue(path) VALUES (?1)",
            [request.root.as_os_str().as_encoded_bytes()],
        )?;
        Ok(Self {
            db,
            directory: None,
            request,
            results: Results::default(),
        })
    }
    fn step(&mut self) -> anyhow::Result<()> {
        self.db.execute_batch("BEGIN")?;
        let started = Instant::now();
        for _ in 0..512 {
            if self.request.cancel.is_cancelled() {
                self.results.done = true;
                self.results.stopped = true;
                break;
            }
            if started.elapsed() >= Duration::from_millis(20) {
                break;
            }
            if self.directory.is_none() {
                let next: Option<(i64, Vec<u8>)> = self
                    .db
                    .query_row("SELECT id,path FROM queue ORDER BY id LIMIT 1", [], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                let Some((id, path)) = next else {
                    self.results.done = true;
                    break;
                };
                self.db.execute("DELETE FROM queue WHERE id=?1", [id])?;
                match std::fs::read_dir(PathBuf::from(std::ffi::OsString::from_vec(path))) {
                    Ok(read) => self.directory = Some(read),
                    Err(_) => {
                        self.results.errors += 1;
                        continue;
                    }
                }
            }
            let Some(entry) = self.directory.as_mut().unwrap().next() else {
                self.directory = None;
                continue;
            };
            self.results.scanned += 1;
            let Ok(entry) = entry else {
                self.results.errors += 1;
                continue;
            };
            let name = entry.file_name();
            if !self.request.hidden && name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let path = entry.path();
            if name
                .to_string_lossy()
                .to_lowercase()
                .contains(&self.request.query)
            {
                self.db
                    .prepare_cached("INSERT INTO hits(path) VALUES (?1)")?
                    .execute([path.as_os_str().as_encoded_bytes()])?;
                self.results.total += 1;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    self.db
                        .prepare_cached("INSERT INTO queue(path) VALUES (?1)")?
                        .execute([path.as_os_str().as_encoded_bytes()])?;
                }
                Err(_) => self.results.errors += 1,
                _ => {}
            }
        }
        self.db.execute_batch("COMMIT")?;
        Ok(())
    }
    fn page(&mut self) -> anyhow::Result<()> {
        let mut statement = self
            .db
            .prepare_cached("SELECT path FROM hits WHERE id > ?1 ORDER BY id LIMIT ?2")?;
        self.results.paths = statement
            .query_map(params![self.results.offset as i64, PAGE_SIZE as i64], |r| {
                let bytes: Vec<u8> = r.get(0)?;
                Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
            })?
            .collect::<Result<_, _>>()?;
        Ok(())
    }
    fn fail(&mut self, error: anyhow::Error) {
        let _ = self.db.execute_batch("ROLLBACK");
        if let Ok(total) = self
            .db
            .query_row("SELECT COUNT(*) FROM hits", [], |r| r.get::<_, i64>(0))
            && let Ok(total) = usize::try_from(total)
        {
            self.results.total = total;
        }
        self.results.failure = Some(format!("Хранилище поиска: {error}"));
        self.results.done = true;
        self.directory = None;
    }
}
impl Default for Search {
    fn default() -> Self {
        let (send, receive) = mpsc::channel();
        let output: Mailbox = Arc::new(Mutex::new(None));
        let mailbox = output.clone();
        thread::spawn(move || {
            let mut run: Option<Run> = None;
            let mut dirty = false;
            let mut last = Instant::now();
            loop {
                let active = run.as_ref().is_some_and(|r| !r.results.done);
                let command = if active {
                    match receive.try_recv() {
                        Ok(c) => Some(c),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(_) => break,
                    }
                } else {
                    match receive.recv() {
                        Ok(c) => Some(c),
                        Err(_) => break,
                    }
                };
                if let Some(command) = command {
                    for command in std::iter::once(command).chain(receive.try_iter()) {
                        match command {
                            Command::Clear => {
                                run = None;
                                dirty = false;
                            }
                            Command::Start(request) if !request.cancel.is_cancelled() => {
                                let id = request.id;
                                run = None; // Delete the obsolete private result store before allocating another.
                                match Run::new(request) {
                                    Ok(new) => {
                                        run = Some(new);
                                        dirty = true;
                                    }
                                    Err(error) => {
                                        *mailbox.lock().unwrap() = Some((
                                            id,
                                            Results {
                                                done: true,
                                                failure: Some(format!("Хранилище поиска: {error}")),
                                                ..Default::default()
                                            },
                                        ));
                                    }
                                }
                            }
                            Command::Page(id, offset) => {
                                if let Some(r) = &mut run
                                    && r.request.id == id
                                {
                                    r.results.offset = offset;
                                    dirty = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let Some(r) = &mut run else {
                    continue;
                };
                if !r.results.done {
                    if let Err(e) = r.step() {
                        r.fail(e);
                    }
                    if r.results.done {
                        dirty = true;
                    }
                }
                if dirty || last.elapsed() >= Duration::from_millis(100) {
                    if let Err(e) = r.page() {
                        r.fail(e);
                    }
                    *mailbox.lock().unwrap() = Some((r.request.id, r.results.clone()));
                    dirty = false;
                    last = Instant::now();
                }
            }
        });
        Self {
            send,
            output,
            cancel: Default::default(),
            wanted: 0,
            id: 0,
            results: Results {
                done: true,
                ..Default::default()
            },
        }
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Search {
    pub fn start(&mut self, root: PathBuf, query: String, hidden: bool) {
        self.cancel.cancel();
        self.cancel = Default::default();
        self.id += 1;
        self.wanted = 0;
        self.results = Results::default();
        if query.trim().is_empty() || query.len() > 4096 {
            let _ = self.send.send(Command::Clear);
            self.results.done = true;
            if query.len() > 4096 {
                self.results.failure = Some("Запрос длиннее 4096 байт".into());
            }
        } else {
            let _ = self.send.send(Command::Start(Request {
                id: self.id,
                root,
                query: query.to_lowercase(),
                hidden,
                cancel: self.cancel.clone(),
            }));
        }
    }
    pub fn request_page(&mut self, offset: usize) {
        if self.wanted != offset {
            self.wanted = offset;
            let _ = self.send.send(Command::Page(self.id, offset));
        }
    }
    pub fn poll(&mut self) {
        if let Some((id, mut results)) = self.output.lock().unwrap().take()
            && id == self.id
            && results.offset == self.wanted
        {
            if self.cancel.is_cancelled() {
                results.stopped = true;
                results.done = true;
            }
            self.results = results;
        }
    }
    pub fn stop(&mut self) {
        self.cancel.cancel();
        self.results.done = true;
        self.results.stopped = true;
        // Wake a finished worker too, while keeping its pages available.
        let _ = self.send.send(Command::Page(self.id, self.wanted));
    }
}
/// Synchronous scanner for isolated fixtures; returns the first result page and total count.
pub fn scan(
    root: &Path,
    query: &str,
    hidden: bool,
    cancel: &Cancellation,
    mut progress: impl FnMut(&Results),
) -> Results {
    let request = Request {
        id: 1,
        root: root.to_path_buf(),
        query: query.to_lowercase(),
        hidden,
        cancel: cancel.clone(),
    };
    let mut run = match Run::new(request) {
        Ok(run) => run,
        Err(e) => {
            return Results {
                done: true,
                failure: Some(e.to_string()),
                ..Default::default()
            };
        }
    };
    while !run.results.done {
        if let Err(e) = run.step() {
            run.fail(e);
        }
        progress(&run.results);
    }
    if let Err(e) = run.page() {
        run.fail(e);
    }
    run.results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_failure_does_not_report_completion_or_rolled_back_matches() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(root.path().join(format!("match-{i}")), b"").unwrap();
        }
        let mut run = Run::new(Request {
            id: 1,
            root: root.path().to_path_buf(),
            query: "match".into(),
            hidden: true,
            cancel: Cancellation::default(),
        })
        .unwrap();
        run.db.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON hits WHEN (SELECT COUNT(*) FROM hits) = 3 BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;").unwrap();
        let error = run.step().unwrap_err();
        run.fail(error);
        run.page().unwrap();
        assert_eq!(run.results.status(), "Прерван");
        assert!(run.results.done);
        assert_eq!(run.results.total, 0);
        assert!(run.results.paths.is_empty());
        assert!(
            run.results
                .failure
                .as_deref()
                .unwrap()
                .contains("simulated storage failure")
        );
    }
}
