//! Visible-file requests, bounded shared cache, no filesystem I/O in the renderer.
use crate::{
    fs::Entry,
    icons::{IconKind, IconSlot},
    operations::Cancellation,
    preview::ImagePreview,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, SystemTime},
};

pub const CAPACITY: usize = 128;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Source {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    request: u64,
}
impl Source {
    pub fn new(entry: &Entry, request: u64) -> Option<Self> {
        let media = matches!(
            IconKind::for_entry(entry),
            IconKind::Image | IconKind::Video
        );
        // .ts is also TypeScript: keep its ordinary code icon unless the worker
        // recognizes a transport-stream signature and actually decodes a frame.
        let transport_stream = !entry.directory
            && !entry.symlink
            && entry
                .path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("ts"));
        (media || transport_stream).then(|| Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
            request,
        })
    }
}
struct Batch {
    generation: u64,
    sources: Vec<Source>,
    cancel: Cancellation,
}
struct Loaded {
    generation: u64,
    source: Source,
    image: Option<Arc<ImagePreview>>,
}
pub struct Thumbnails {
    wake: mpsc::SyncSender<()>,
    inbox: Arc<Mutex<Option<Batch>>>,
    results: mpsc::Receiver<Loaded>,
    wanted: Vec<Source>,
    pending: HashSet<Source>,
    cache: HashMap<Source, (u64, Option<Arc<ImagePreview>>)>,
    cancel: Cancellation,
    generation: u64,
}
impl Default for Thumbnails {
    fn default() -> Self {
        let (wake, receive) = mpsc::sync_channel(1);
        let inbox: Arc<Mutex<Option<Batch>>> = Arc::default();
        let mailbox = inbox.clone();
        let (send, results) = mpsc::sync_channel(4);
        std::thread::spawn(move || {
            while receive.recv().is_ok() {
                let Some(batch) = mailbox.lock().unwrap().take() else {
                    continue;
                };
                'batch: for source in batch.sources {
                    if batch.cancel.is_cancelled() {
                        break;
                    }
                    let image = load(&source, &batch.cancel);
                    if batch.cancel.is_cancelled() {
                        break;
                    }
                    let mut message = Loaded {
                        generation: batch.generation,
                        source,
                        image,
                    };
                    loop {
                        match send.try_send(message) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Disconnected(_)) => return,
                            Err(mpsc::TrySendError::Full(value)) => {
                                message = value;
                            }
                        }
                        if batch.cancel.is_cancelled() {
                            break 'batch;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        });
        Self {
            wake,
            inbox,
            results,
            wanted: Vec::new(),
            pending: HashSet::new(),
            cache: HashMap::new(),
            cancel: Cancellation::default(),
            generation: 0,
        }
    }
}
impl Drop for Thumbnails {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Thumbnails {
    pub fn loading(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn poll(&mut self) {
        while let Ok(result) = self.results.try_recv() {
            if result.generation != self.generation || !self.pending.remove(&result.source) {
                continue;
            }
            if self.cache.len() >= CAPACITY
                && !self.cache.contains_key(&result.source)
                && let Some(old) = self
                    .cache
                    .iter()
                    .filter(|(k, _)| !self.wanted.contains(k))
                    .min_by_key(|(_, (age, _))| *age)
                    .map(|(key, _)| key.clone())
            {
                self.cache.remove(&old);
            }
            self.cache
                .insert(result.source, (self.generation, result.image));
        }
    }
    pub fn update(&mut self, slots: &mut [IconSlot], enabled: bool) {
        self.poll();
        let mut seen = HashSet::new();
        let wanted: Vec<_> = slots
            .iter()
            .filter_map(|s| s.source.clone())
            .filter(|s| enabled && seen.insert(s.clone()))
            .take(CAPACITY)
            .collect();
        if wanted != self.wanted {
            self.cancel.cancel();
            self.cancel = Cancellation::default();
            self.generation += 1;
            self.wanted = wanted;
            self.pending = self
                .wanted
                .iter()
                .filter(|s| !self.cache.contains_key(*s))
                .cloned()
                .collect();
            let sources = self
                .wanted
                .iter()
                .filter(|s| self.pending.contains(*s))
                .cloned()
                .collect();
            *self.inbox.lock().unwrap() = Some(Batch {
                generation: self.generation,
                sources,
                cancel: self.cancel.clone(),
            });
            let _ = self.wake.try_send(());
        }
        if !enabled {
            self.cache.clear();
        }
        for slot in slots {
            if let Some(source) = &slot.source
                && enabled
                && self.wanted.contains(source)
                && let Some((age, image)) = self.cache.get_mut(source)
            {
                *age = self.generation;
                slot.content = image.clone();
            }
        }
    }
    pub fn cached_frames(&self) -> usize {
        self.cache.values().filter(|(_, v)| v.is_some()).count()
    }
}

#[cfg(any(feature = "guitui", feature = "tui"))]
fn load(source: &Source, cancel: &Cancellation) -> Option<Arc<ImagePreview>> {
    use std::os::unix::fs::MetadataExt;
    let before = std::fs::symlink_metadata(&source.path).ok()?;
    if !before.is_file()
        || before.len() != source.size
        || before.modified().ok() != source.modified
        || cancel.is_cancelled()
    {
        return None;
    }
    let preview = crate::fs::preview_until(&source.path, true, cancel)
        .ok()?
        .image?;
    if cancel.is_cancelled() {
        return None;
    }
    let after = std::fs::symlink_metadata(&source.path).ok()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&before) != stamp(&after) {
        return None;
    }
    let image = image::RgbaImage::from_raw(preview.width, preview.height, preview.pixels.clone())?;
    let small = image::DynamicImage::ImageRgba8(image)
        .thumbnail(160, 160)
        .into_rgba8();
    Some(Arc::new(
        ImagePreview::new(small.width(), small.height(), small.into_raw()).ok()?,
    ))
}
#[cfg(not(any(feature = "guitui", feature = "tui")))]
fn load(_: &Source, _: &Cancellation) -> Option<Arc<ImagePreview>> {
    None
}
