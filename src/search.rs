//! Complete name search with a disk-backed directory queue and paged results.
use crate::operations::Cancellation;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    os::{fd::AsRawFd, unix::ffi::OsStringExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
pub const PAGE_SIZE: usize = 128;
#[derive(Clone, Default)]
pub struct Results {
    pub paths: Vec<PathBuf>,
    pub entries: Vec<crate::fs::Entry>,
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
    options: crate::filter::Options,
    order: Option<(crate::fs::Sort, crate::grouping::Grouping)>,
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
    directory_fd: Option<std::fs::File>,
    directory_path: PathBuf,
    depth: usize,
    owners: std::collections::BTreeMap<u32, String>,
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
            CREATE TABLE queue (id INTEGER PRIMARY KEY, path BLOB NOT NULL, depth INTEGER NOT NULL);
            CREATE TABLE hits (id INTEGER PRIMARY KEY, path BLOB NOT NULL, payload BLOB, gkey TEXT, skey TEXT, isdir INTEGER, name TEXT);")?;
        db.execute(
            "INSERT INTO queue(path,depth) VALUES (?1,0)",
            [request.root.as_os_str().as_encoded_bytes()],
        )?;
        if let Some((sort, group)) = request.order {
            let direction = |descending| if descending { "DESC" } else { "ASC" };
            db.execute_batch(&format!("CREATE INDEX hit_order ON hits(gkey IS NULL,gkey {},isdir DESC,skey IS NULL,skey {},name,path)", direction(group.descending), direction(sort.field().1)))?;
        }
        Ok(Self {
            db,
            directory: None,
            directory_fd: None,
            directory_path: PathBuf::new(),
            depth: 0,
            owners: if request.order.is_some() {
                crate::fs::local_owners()
            } else {
                Default::default()
            },
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
                let next: Option<(i64, Vec<u8>, i64)> = self
                    .db
                    .query_row(
                        "SELECT id,path,depth FROM queue ORDER BY id LIMIT 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let Some((id, path, depth)) = next else {
                    self.results.done = true;
                    break;
                };
                self.db.execute("DELETE FROM queue WHERE id=?1", [id])?;
                self.depth = depth as usize;
                self.directory_path = PathBuf::from(std::ffi::OsString::from_vec(path));
                match pin_directory(&self.directory_path, &self.request.cancel).and_then(|fd| {
                    let read = std::fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd()))?;
                    Ok((fd, read))
                }) {
                    Ok((fd, read)) => {
                        self.directory = Some(read);
                        self.directory_fd = Some(fd);
                    }
                    Err(_) => {
                        self.results.errors += 1;
                        continue;
                    }
                }
            }
            let Some(entry) = self.directory.as_mut().unwrap().next() else {
                self.directory = None;
                self.directory_fd = None;
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
            let path = self.directory_path.join(&name);
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    self.results.errors += 1;
                    continue;
                }
            };
            let directory = kind.is_dir() || (kind.is_symlink() && entry.path().is_dir());
            if self.request.options.matches_name(
                &name.to_string_lossy(),
                directory,
                &self.request.query,
            ) {
                if let Some((sort, group)) = self.request.order {
                    match crate::fs::search_entry(path.clone(), &entry.path(), &self.owners) {
                        Ok(e) => {
                            let data = serde_json::to_vec(&(
                                &e.name,
                                e.directory,
                                e.symlink,
                                e.hidden,
                                e.size,
                                e.modified,
                                &e.details,
                            ))?;
                            self.db.prepare_cached("INSERT INTO hits(path,payload,gkey,skey,isdir,name) VALUES (?1,?2,?3,?4,?5,?6)")?.execute(params![path.as_os_str().as_encoded_bytes(),data,group.order_key(&e),sort.field().0.order_key(&e),e.directory,e.name.to_lowercase()])?;
                            self.results.total += 1;
                        }
                        Err(_) => self.results.errors += 1,
                    }
                } else {
                    self.db
                        .prepare_cached("INSERT INTO hits(path) VALUES (?1)")?
                        .execute([path.as_os_str().as_encoded_bytes()])?;
                    self.results.total += 1;
                }
            }
            if kind.is_dir()
                && self
                    .request
                    .options
                    .depth
                    .is_none_or(|max| self.depth < usize::from(max))
            {
                self.db
                    .prepare_cached("INSERT INTO queue(path,depth) VALUES (?1,?2)")?
                    .execute(params![
                        path.as_os_str().as_encoded_bytes(),
                        (self.depth + 1) as i64
                    ])?;
            }
        }
        self.db.execute_batch("COMMIT")?;
        Ok(())
    }
    fn page(&mut self) -> anyhow::Result<()> {
        if let Some((sort, group)) = self.request.order {
            let direction = |descending| if descending { "DESC" } else { "ASC" };
            // Sort the complete disk-backed catalog, never just the loaded page.
            let sql = format!(
                "SELECT path,payload FROM hits ORDER BY gkey IS NULL,gkey {},isdir DESC,skey IS NULL,skey {},name,path LIMIT ?1 OFFSET ?2",
                direction(group.descending),
                direction(sort.field().1)
            );
            let mut statement = self.db.prepare_cached(&sql)?;
            self.results.entries = statement
                .query_map(params![PAGE_SIZE as i64, self.results.offset as i64], |r| {
                    Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?
                .map(|row| {
                    let (path, data) = row?;
                    let (name, directory, symlink, hidden, size, modified, details) =
                        serde_json::from_slice(&data)?;
                    Ok(crate::fs::Entry {
                        path: PathBuf::from(std::ffi::OsString::from_vec(path)),
                        name,
                        directory,
                        symlink,
                        hidden,
                        size,
                        modified,
                        details,
                    })
                })
                .collect::<anyhow::Result<_>>()?;
            self.results.paths = self
                .results
                .entries
                .iter()
                .map(|e| e.path.clone())
                .collect();
        } else {
            let mut statement = self
                .db
                .prepare_cached("SELECT path FROM hits WHERE id > ?1 ORDER BY id LIMIT ?2")?;
            self.results.paths = statement
                .query_map(params![self.results.offset as i64, PAGE_SIZE as i64], |r| {
                    let bytes: Vec<u8> = r.get(0)?;
                    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
                })?
                .collect::<Result<_, _>>()?;
        }
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
        self.directory_fd = None;
    }
}

// Reopen queued paths one component at a time without following replaced links.
// Only one directory is held while scanning, even beyond PATH_MAX or depth 64.
fn pin_directory(path: &Path, cancel: &Cancellation) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    use std::{fs::File, path::Component};
    let mut fd = File::from(rustix::fs::open(
        if path.is_absolute() { "/" } else { "." },
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for part in path.components() {
        if cancel.is_cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                fd = File::from(rustix::fs::openat(
                    &fd,
                    name,
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?)
            }
            _ => return Err(std::io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(fd)
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
        self.start_request(
            root,
            query,
            hidden,
            crate::filter::Options {
                depth: None,
                ..Default::default()
            },
            None,
        );
    }
    pub fn start_filter(
        &mut self,
        root: PathBuf,
        query: String,
        hidden: bool,
        options: crate::filter::Options,
        sort: crate::fs::Sort,
        group: crate::grouping::Grouping,
    ) {
        self.start_request(root, query, hidden, options, Some((sort, group)));
    }
    fn start_request(
        &mut self,
        root: PathBuf,
        query: String,
        hidden: bool,
        options: crate::filter::Options,
        order: Option<(crate::fs::Sort, crate::grouping::Grouping)>,
    ) {
        self.cancel.cancel();
        self.cancel = Default::default();
        self.id += 1;
        self.wanted = 0;
        self.results = Results::default();
        if (order.is_none() && query.trim().is_empty())
            || query.len() > 4096
            || options.extension.len() > 4096
        {
            let _ = self.send.send(Command::Clear);
            self.results.done = true;
            if query.len() > 4096 || options.extension.len() > 4096 {
                self.results.failure = Some("Запрос длиннее 4096 байт".into());
            }
        } else {
            let _ = self.send.send(Command::Start(Request {
                id: self.id,
                root,
                query,
                hidden,
                options,
                order,
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
        query: query.into(),
        hidden,
        options: crate::filter::Options {
            depth: None,
            ..Default::default()
        },
        order: None,
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

    fn filtered_request(root: PathBuf) -> Request {
        Request {
            id: 1,
            root,
            query: "needle".into(),
            hidden: true,
            options: crate::filter::Options {
                depth: None,
                ..Default::default()
            },
            order: Some((
                crate::fs::Sort::default(),
                crate::grouping::Grouping::default(),
            )),
            cancel: Cancellation::default(),
        }
    }

    #[test]
    fn queued_directory_replaced_with_a_link_is_not_traversed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("needle"), "unrelated").unwrap();
        let queued = root.path().join("queued");
        std::fs::create_dir(&queued).unwrap();
        let mut run = Run::new(filtered_request(root.path().to_path_buf())).unwrap();
        run.db.execute_batch("DELETE FROM queue").unwrap();
        run.db
            .execute(
                "INSERT INTO queue(path,depth) VALUES (?1,1)",
                [queued.as_os_str().as_encoded_bytes()],
            )
            .unwrap();
        std::fs::remove_dir(&queued).unwrap();
        std::os::unix::fs::symlink(outside.path(), &queued).unwrap();
        while !run.results.done {
            run.step().unwrap();
        }
        run.page().unwrap();
        assert_eq!(run.results.errors, 1);
        assert_eq!(run.results.total, 0);
        assert!(run.results.entries.is_empty());
    }

    #[test]
    fn filtered_pages_preserve_raw_names_beyond_path_max_and_depth_64() {
        use rustix::fs::{Mode, OFlags};
        use std::fs::File;
        let root = tempfile::tempdir().unwrap();
        let mut path = root.path().to_path_buf();
        let mut fd = File::open(&path).unwrap();
        for _ in 0..70 {
            let name = "d".repeat(80);
            rustix::fs::mkdirat(&fd, &name, Mode::RWXU).unwrap();
            fd = File::from(
                rustix::fs::openat(
                    &fd,
                    &name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .unwrap(),
            );
            path.push(name);
        }
        let mut expected = vec![];
        for i in 0..140 {
            let name = std::ffi::OsString::from_vec(
                format!("needle-{i:03}-").bytes().chain([0xff]).collect(),
            );
            drop(
                rustix::fs::openat(
                    &fd,
                    &name,
                    OFlags::CREATE | OFlags::WRONLY | OFlags::EXCL | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .unwrap(),
            );
            expected.push(path.join(name));
        }
        assert!(path.as_os_str().len() > 4096);
        let mut run = Run::new(filtered_request(root.path().to_path_buf())).unwrap();
        while !run.results.done {
            run.step().unwrap();
        }
        assert_eq!(run.results.errors, 0);
        assert_eq!(run.results.total, 140);
        let mut actual = vec![];
        for offset in [0, PAGE_SIZE] {
            run.results.offset = offset;
            run.page().unwrap();
            actual.extend(run.results.entries.iter().map(|e| e.path.clone()));
        }
        assert_eq!(actual, expected);
    }

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
            options: crate::filter::Options {
                depth: None,
                ..Default::default()
            },
            order: None,
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
