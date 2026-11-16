//! Disk scanning and cleaning. UI-free: workers write into a shared
//! [`Progress`] that the UI samples every frame.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Apps,
    Developer,
    Photos,
    Movies,
    Documents,
    Music,
    System,
    Caches,
    Trash,
}

impl Category {
    /// Bar order, left to right. The legend reads it row by row.
    pub const ALL: [Category; 9] = [
        Category::Apps,
        Category::Developer,
        Category::Photos,
        Category::Movies,
        Category::Documents,
        Category::Music,
        Category::System,
        Category::Caches,
        Category::Trash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Apps => "Apps",
            Category::Developer => "Developer",
            Category::Photos => "Photos",
            Category::Movies => "Movies",
            Category::Documents => "Documents",
            Category::Music => "Music",
            Category::System => "System data",
            Category::Caches => "Caches",
            Category::Trash => "Trash",
        }
    }
}

/// Something the app offers to clear, in the order it offers them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cleanable {
    Caches,
    Trash,
}

impl Cleanable {
    pub const ALL: [Cleanable; 2] = [Cleanable::Caches, Cleanable::Trash];

    pub fn category(self) -> Category {
        match self {
            Cleanable::Caches => Category::Caches,
            Cleanable::Trash => Category::Trash,
        }
    }

    fn root(self, home: &Path) -> PathBuf {
        match self {
            Cleanable::Caches => home.join("Library/Caches"),
            Cleanable::Trash => home.join(".Trash"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Volume {
    pub total: u64,
    pub used: u64,
}

pub fn volume() -> Volume {
    let path = CString::new("/").unwrap();
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
        return Volume { total: 0, used: 0 };
    }
    let block = stat.f_bsize as u64;
    let total = stat.f_blocks * block;
    Volume {
        total,
        used: total.saturating_sub(stat.f_bavail * block),
    }
}

/// Live state of a scan or clean, shared between a worker and the UI.
#[derive(Default)]
pub struct Progress {
    bytes: [AtomicU64; 9],
    freed: AtomicU64,
    done: AtomicBool,
    current: Mutex<String>,
    largest_size: [AtomicU64; 9],
    largest: Mutex<[Option<(String, u64)>; 9]>,
    groups: Mutex<Vec<(Category, String, Arc<AtomicU64>)>>,
    simulators: AtomicU64,
    build_output: AtomicU64,
}

/// What a finished scan learned beyond category totals, used to explain them.
#[derive(Clone, Default)]
pub struct Report {
    /// Largest single file per category.
    pub largest_file: [Option<(String, u64)>; 9],
    /// Largest top-level item (an app, a project folder) per category.
    pub largest_item: [Option<(String, u64)>; 9],
    pub simulators: u64,
    pub build_output: u64,
}

impl Progress {
    pub fn bytes(&self, category: Category) -> u64 {
        self.bytes[category as usize].load(Relaxed)
    }

    pub fn freed(&self) -> u64 {
        self.freed.load(Relaxed)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn report(&self) -> Report {
        let mut largest_item: [Option<(String, u64)>; 9] = Default::default();
        for (category, name, bytes) in self.groups.lock().unwrap().iter() {
            let bytes = bytes.load(Relaxed);
            let slot = &mut largest_item[*category as usize];
            if slot.as_ref().is_none_or(|(_, best)| bytes > *best) {
                *slot = Some((name.clone(), bytes));
            }
        }
        Report {
            largest_file: self.largest.lock().unwrap().clone(),
            largest_item,
            simulators: self.simulators.load(Relaxed),
            build_output: self.build_output.load(Relaxed),
        }
    }

    fn add(&self, category: Category, bytes: u64) {
        self.bytes[category as usize].fetch_add(bytes, Relaxed);
    }

    fn remove(&self, category: Category, bytes: u64) {
        let _ = self.bytes[category as usize]
            .fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(bytes)));
        self.freed.fetch_add(bytes, Relaxed);
    }

    /// Updates the path shown in the UI without ever blocking a worker.
    fn show(&self, path: &Path) {
        if let Ok(mut current) = self.current.try_lock() {
            current.clear();
            current.push_str(&path.to_string_lossy());
        }
    }

    fn note_file(&self, category: Category, bytes: u64, name: impl FnOnce() -> String) {
        let i = category as usize;
        if self.largest_size[i].fetch_max(bytes, Relaxed) < bytes {
            let mut largest = self.largest.lock().unwrap();
            if largest[i].as_ref().is_none_or(|(_, best)| bytes > *best) {
                largest[i] = Some((name(), bytes));
            }
        }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/".into())
}

/// Where each category lives. System data is whatever the disk uses beyond these.
fn roots(home: &Path) -> Vec<(PathBuf, Category)> {
    let h = |p: &str| home.join(p);
    let mut roots = vec![
        ("/Applications".into(), Category::Apps),
        (h("Applications"), Category::Apps),
        (h("Library/Developer"), Category::Developer),
        (h("Pictures"), Category::Photos),
        (h("Movies"), Category::Movies),
        (h("Documents"), Category::Documents),
        (h("Desktop"), Category::Documents),
        (h("Downloads"), Category::Documents),
        (h("Music"), Category::Music),
        (h("Library/Caches"), Category::Caches),
        (h(".Trash"), Category::Trash),
    ];
    for dev in [
        "Developer",
        "Projects",
        ".cargo",
        ".rustup",
        ".npm",
        ".bun",
        ".gradle",
        ".m2",
        ".cocoapods",
        "Library/pnpm",
    ] {
        roots.push((h(dev), Category::Developer));
    }
    roots
}

const SIMULATORS: u8 = 1;
const BUILD_OUTPUT: u8 = 2;

fn tags_for(path: &Path, name: &str) -> u8 {
    match name {
        "CoreSimulator" => SIMULATORS,
        "DerivedData" => BUILD_OUTPUT,
        // Cargo marks its target dirs; plain "target" folders may be anything.
        "target" if path.join("CACHEDIR.TAG").exists() => BUILD_OUTPUT,
        _ => 0,
    }
}

struct Walker<'a> {
    progress: &'a Progress,
    /// Hard-linked files are counted once.
    seen: Mutex<HashSet<(u64, u64)>>,
    dirs: AtomicU64,
}

impl Walker<'_> {
    fn file_size(&self, md: &fs::Metadata) -> Option<u64> {
        if md.nlink() > 1 && !self.seen.lock().unwrap().insert((md.dev(), md.ino())) {
            return None;
        }
        // Allocated blocks, not logical length: sparse and cloud files take less.
        Some(md.blocks() * 512)
    }
}

fn walk<'s>(
    scope: &rayon::Scope<'s>,
    walker: &'s Walker<'s>,
    dir: PathBuf,
    dev: u64,
    category: Category,
    tags: u8,
    group: Arc<AtomicU64>,
) {
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    if walker.dirs.fetch_add(1, Relaxed) % 16 == 0 {
        walker.progress.show(&dir);
    }
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let Ok(md) = entry.metadata() else { continue };
        if md.dev() != dev {
            continue;
        }
        if kind.is_dir() {
            let path = entry.path();
            let tags = tags | tags_for(&path, &entry.file_name().to_string_lossy());
            let group = group.clone();
            scope.spawn(move |scope| walk(scope, walker, path, dev, category, tags, group));
        } else if let Some(size) = walker.file_size(&md) {
            total += size;
            if kind.is_file() {
                walker.progress.note_file(category, size, || {
                    entry.file_name().to_string_lossy().into_owned()
                });
            }
        }
    }
    let progress = walker.progress;
    progress.add(category, total);
    group.fetch_add(total, Relaxed);
    if tags & SIMULATORS != 0 {
        progress.simulators.fetch_add(total, Relaxed);
    }
    if tags & BUILD_OUTPUT != 0 {
        progress.build_output.fetch_add(total, Relaxed);
    }
}

/// Scans every category root in parallel. Returns immediately.
pub fn scan(progress: Arc<Progress>) {
    thread::spawn(move || {
        let home = home();
        // Directory reads block on IO, so oversubscribe the cores.
        let threads = thread::available_parallelism().map_or(8, |n| n.get() * 2);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let walker = Walker {
            progress: &progress,
            seen: Mutex::default(),
            dirs: AtomicU64::new(0),
        };
        let walker = &walker;
        pool.scope(|scope| {
            for (root, category) in roots(&home) {
                let Ok(md) = fs::metadata(&root) else {
                    continue;
                };
                let dev = md.dev();
                let Ok(entries) = fs::read_dir(&root) else {
                    continue;
                };
                // Each top-level item gets its own counter so we can name the biggest one.
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let group = Arc::new(AtomicU64::new(0));
                    walker.progress.groups.lock().unwrap().push((
                        category,
                        name.clone(),
                        group.clone(),
                    ));
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    let Ok(md) = entry.metadata() else { continue };
                    if kind.is_dir() && md.dev() == dev {
                        let path = entry.path();
                        let tags = tags_for(&path, &name);
                        scope.spawn(move |scope| {
                            walk(scope, walker, path, dev, category, tags, group)
                        });
                    } else if !kind.is_dir()
                        && let Some(size) = walker.file_size(&md)
                    {
                        walker.progress.add(category, size);
                        group.fetch_add(size, Relaxed);
                        if kind.is_file() {
                            walker.progress.note_file(category, size, || name);
                        }
                    }
                }
            }
        });
        progress.done.store(true, Relaxed);
    });
}

/// Deletes everything inside the cleanable's folder, keeping the folder itself.
/// Never follows symlinks. Returns immediately.
pub fn clean(target: Cleanable, progress: Arc<Progress>) {
    progress.done.store(false, Relaxed);
    thread::spawn(move || {
        let root = target.root(&home());
        let category = target.category();
        if let Ok(entries) = fs::read_dir(&root) {
            let entries: Vec<_> = entries.flatten().collect();
            use rayon::prelude::*;
            entries
                .par_iter()
                .for_each(|entry| remove(&entry.path(), &progress, category));
        }
        progress.done.store(true, Relaxed);
    });
}

fn remove(path: &Path, progress: &Progress, category: Category) {
    // symlink_metadata: a symlink is removed as a link, never followed.
    let Ok(md) = fs::symlink_metadata(path) else {
        return;
    };
    if md.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                remove(&entry.path(), progress, category);
            }
        }
        let _ = fs::remove_dir(path);
    } else {
        progress.show(path);
        if fs::remove_file(path).is_ok() {
            progress.remove(category, md.blocks() * 512);
        }
    }
}

/// A scripted scan and clean with sample data, for trying the app without
/// touching the disk.
pub mod demo {
    use super::*;

    const GB: f64 = 1e9;

    pub const VOLUME: Volume = Volume {
        total: 512_000_000_000,
        used: 357_800_000_000,
    };

    const SIZES: [f64; 9] = [52.6, 36.3, 58.6, 44.1, 41.7, 12.1, 0.0, 46.8, 31.4];

    const PATHS: [&str; 12] = [
        "/Applications/Utilities",
        "/Applications/Northwind.app/Contents",
        "~/Library/Developer/CoreSimulator/Devices",
        "~/Library/Developer/Xcode/DerivedData",
        "~/Pictures/Photos Library.photoslibrary",
        "~/Movies/Exports",
        "~/Documents/Taxes",
        "~/Desktop/Screenshots",
        "~/Music/Music/Media",
        "~/Library/Logs",
        "~/Library/Caches/com.penrose.studio",
        "~/.Trash/Old exports",
    ];

    pub fn scan(progress: Arc<Progress>) {
        thread::spawn(move || {
            let steps = 120;
            for step in 1..=steps {
                let t = step as f64 / steps as f64;
                for (i, size) in SIZES.iter().enumerate() {
                    // Categories finish one after another, overlapping a little.
                    let start = i as f64 / 9.0 * 0.65;
                    let local = ((t - start) / 0.35).clamp(0.0, 1.0);
                    let want = (size * GB * local) as u64;
                    progress.bytes[i].store(want, Relaxed);
                }
                *progress.current.lock().unwrap() = PATHS[(t * 11.0) as usize].to_string();
                thread::sleep(Duration::from_millis(18));
            }
            *progress.largest.lock().unwrap() = {
                let mut largest: [Option<(String, u64)>; 9] = Default::default();
                largest[Category::Photos as usize] =
                    Some(("Trip to Lisbon.heic".into(), 410_000_000));
                largest[Category::Movies as usize] =
                    Some(("Screen Recording 2026-10-02.mov".into(), 6_200_000_000));
                largest[Category::Documents as usize] =
                    Some(("Northwind lookbook.pdf".into(), 3_800_000_000));
                largest[Category::Music as usize] =
                    Some(("Live at the Roundhouse.flac".into(), 1_100_000_000));
                largest
            };
            progress.groups.lock().unwrap().push((
                Category::Apps,
                "Xcode.app".into(),
                Arc::new(AtomicU64::new(12_100_000_000)),
            ));
            progress.simulators.store(17_200_000_000, Relaxed);
            progress.build_output.store(8_100_000_000, Relaxed);
            progress.done.store(true, Relaxed);
        });
    }

    pub fn clean(target: Cleanable, progress: Arc<Progress>) {
        progress.done.store(false, Relaxed);
        thread::spawn(move || {
            let category = target.category();
            let start = progress.bytes(category);
            // Some caches are in use and stay behind.
            let keep = if target == Cleanable::Caches {
                1_200_000_000
            } else {
                0
            };
            let names = match target {
                Cleanable::Caches => [
                    "com.penrose.studio",
                    "Northwind Mail",
                    "com.apple.Safari",
                    "Homebrew",
                ],
                Cleanable::Trash => [
                    "Screen recording 2026-08-14.mov",
                    "Old exports",
                    "Archive.zip",
                    "Untitled",
                ],
            };
            let steps = 70;
            for step in 1..=steps {
                let t = step as f64 / steps as f64;
                let left = keep + ((start - keep) as f64 * (1.0 - t)) as u64;
                let removed = progress.bytes(category) - left;
                progress.remove(category, removed);
                let dir = if target == Cleanable::Caches {
                    "~/Library/Caches"
                } else {
                    "~/.Trash"
                };
                *progress.current.lock().unwrap() = format!("{dir}/{}", names[(t * 3.99) as usize]);
                thread::sleep(Duration::from_millis(18));
            }
            progress.done.store(true, Relaxed);
        });
    }
}
