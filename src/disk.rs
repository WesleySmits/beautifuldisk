//! Disk scanning and cleaning. UI-free: workers write into a shared
//! [`Progress`] that the UI samples every frame.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::bulk;
use crate::clutter::{self, Item};
use crate::tree::Tree;

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

#[derive(Clone, Copy)]
pub struct Volume {
    pub total: u64,
    pub used: u64,
}

pub fn volume() -> Volume {
    let path = CString::new("/").unwrap();
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Volume { total: 0, used: 0 };
    }
    let block = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block;
    Volume {
        total,
        used: total.saturating_sub(stat.f_bavail as u64 * block),
    }
}

/// Live state of a scan or clean, shared between a worker and the UI.
#[derive(Default)]
pub struct Progress {
    bytes: [AtomicU64; 9],
    freed: AtomicU64,
    done: AtomicBool,
    current: Mutex<String>,
    report: Mutex<Report>,
    /// Cleanup items, set once detection finishes after the scan.
    items: Mutex<Option<Arc<Vec<Item>>>>,
    /// Bit `i` is set while item `i` is being cleared.
    cleaning: AtomicU64,
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
        self.report.lock().unwrap().clone()
    }

    pub(crate) fn add(&self, category: Category, bytes: u64) {
        self.bytes[category as usize].fetch_add(bytes, Relaxed);
    }

    pub fn items(&self) -> Option<Arc<Vec<Item>>> {
        self.items.lock().unwrap().clone()
    }

    fn set_items(&self, items: Vec<Item>) {
        *self.items.lock().unwrap() = Some(Arc::new(items));
    }

    /// Items being cleared right now, one bit each (items past 64 never show).
    pub fn cleaning(&self) -> u64 {
        self.cleaning.load(Relaxed)
    }

    pub(crate) fn set_cleaning(&self, item: usize, on: bool) {
        let Some(bit) = 1u64.checked_shl(item as u32) else {
            return;
        };
        if on {
            self.cleaning.fetch_or(bit, Relaxed);
        } else {
            self.cleaning.fetch_and(!bit, Relaxed);
        }
    }

    pub(crate) fn start_cleaning(&self) {
        self.done.store(false, Relaxed);
    }

    pub(crate) fn finish_cleaning(&self) {
        self.done.store(true, Relaxed);
    }

    pub(crate) fn set_system(&self, bytes: u64) {
        self.bytes[Category::System as usize].store(bytes, Relaxed);
    }

    pub(crate) fn remove(&self, category: Category, bytes: u64) {
        let _ = self.bytes[category as usize]
            .fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(bytes)));
        self.freed.fetch_add(bytes, Relaxed);
    }

    /// Updates the path shown in the UI without ever blocking a worker.
    pub(crate) fn show(&self, path: &Path) {
        if let Ok(mut current) = self.current.try_lock() {
            current.clear();
            current.push_str(&path.to_string_lossy());
        }
    }

    fn finish(&self, totals: [u64; 9], report: Report) {
        for (bytes, total) in self.bytes.iter().zip(totals) {
            bytes.store(total, Relaxed);
        }
        *self.report.lock().unwrap() = report;
        self.done.store(true, Relaxed);
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/".into())
}

/// Where each category lives. System data is whatever the disk uses beyond these.
pub(crate) fn roots(home: &Path) -> Vec<(PathBuf, Category)> {
    let h = |p: &str| home.join(p);
    #[cfg(target_os = "macos")]
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
    // Installed software lives in /usr and /opt plus Flatpak and Snap.
    #[cfg(not(target_os = "macos"))]
    let mut roots = vec![
        ("/usr".into(), Category::Apps),
        ("/opt".into(), Category::Apps),
        ("/var/lib/flatpak".into(), Category::Apps),
        ("/snap".into(), Category::Apps),
        (h(".local/share/flatpak"), Category::Apps),
        (h("Pictures"), Category::Photos),
        (h("Videos"), Category::Movies),
        (h("Documents"), Category::Documents),
        (h("Desktop"), Category::Documents),
        (h("Downloads"), Category::Documents),
        (h("Music"), Category::Music),
        (h(".cache"), Category::Caches),
        (h(".local/share/Trash"), Category::Trash),
    ];
    for dev in clutter::EXTRA_ROOTS.iter().copied().chain([
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
    ]) {
        let path = h(dev);
        // ~/.cache is already the Caches root on Linux.
        if roots.iter().all(|(root, _)| *root != path) {
            roots.push((path, Category::Developer));
        }
    }
    roots
}

/// Scans every category root. Returns immediately.
///
/// The first scan walks every folder. Later scans load the saved tree and
/// re-read only folders FSEvents reports as changed since it was saved.
pub fn scan(progress: Arc<Progress>) {
    thread::spawn(move || {
        bulk::raise_fd_limit();
        let home = home();
        let tree = match Tree::load(&home) {
            Some(mut tree) => {
                if tree.refresh(&home, &progress) {
                    tree
                } else {
                    Tree::scan(&home, &progress)
                }
            }
            None => Tree::scan(&home, &progress),
        };
        progress.finish(tree.totals(), tree.report());
        progress.set_items(clutter::detect(&tree.index(), &home));
        if let Err(err) = tree.save(&home) {
            eprintln!("could not save scan: {err}");
        }
    });
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

    const SIZES: [f64; 9] = [52.6, 112.0, 38.6, 24.1, 31.7, 12.1, 0.0, 22.0, 6.4];

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
            let mut report = Report::default();
            let mut file = |c: Category, name: &str, bytes: u64| {
                report.largest_file[c as usize] = Some((name.into(), bytes));
            };
            file(Category::Photos, "Trip to Lisbon.heic", 410_000_000);
            file(
                Category::Movies,
                "Screen Recording 2026-10-02.mov",
                6_200_000_000,
            );
            file(Category::Documents, "Northwind lookbook.pdf", 3_800_000_000);
            file(
                Category::Music,
                "Live at the Roundhouse.flac",
                1_100_000_000,
            );
            report.largest_item[Category::Apps as usize] =
                Some(("Xcode.app".into(), 12_100_000_000));
            report.simulators = 17_200_000_000;
            report.build_output = 8_100_000_000;
            *progress.report.lock().unwrap() = report;
            progress.done.store(true, Relaxed);
            // Detection takes a moment after the scan on a real disk too.
            thread::sleep(Duration::from_millis(600));
            progress.set_items(clutter::demo());
        });
    }
}
