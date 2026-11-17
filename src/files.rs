//! The user's own files worth a second look: copies of the same file, and
//! screenshots older than three months. UI-free, like `disk`: a worker fills
//! a shared [`Search`] that the UI samples every frame.
//!
//! Only folders people put files in by hand are read (never app data or
//! code), because deleting one copy of something an app or project uses
//! breaks it. Removing moves files to the Trash, so they can be put back.

use std::collections::HashMap;
use std::fs::{self, File as FsFile};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};

use rayon::prelude::*;

use crate::{access, disk, tree};

/// Smaller copies are not worth a tile.
const MIN_COPY: u64 = 1_000_000;
/// Screenshots older than this count as old.
const OLD: Duration = Duration::from_secs(90 * 24 * 3600);
/// Bytes read from each end of a file before reading all of it.
const EDGE: u64 = 16 * 1024;
/// Entries that get a picture. The rest share one tile.
pub const SHOWN: usize = 36;

#[derive(Clone)]
pub struct File {
    pub path: PathBuf,
    /// Bytes on disk.
    pub size: u64,
    pub modified: SystemTime,
}

pub enum Kind {
    /// The same bytes in several places. `files[0]` is the one kept unless
    /// the user picks another.
    Copies(Vec<File>),
    Screenshot(File),
}

pub struct Entry {
    pub kind: Kind,
    /// What trashing it frees: every copy but the kept one (clones that
    /// share their blocks free nothing), or the whole screenshot.
    pub bytes: u64,
}

impl Entry {
    pub fn files(&self) -> &[File] {
        match &self.kind {
            Kind::Copies(files) => files,
            Kind::Screenshot(file) => std::slice::from_ref(file),
        }
    }

    pub fn name(&self) -> String {
        file_name(&self.files()[0].path)
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// A small picture of a file, colors row by row.
pub struct Thumb {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<[u8; 3]>,
}

impl Thumb {
    /// Color at `u`, `v` (0 to 1) with the picture filling a `aspect`
    /// (width / height) box, cropped to fit.
    pub fn at(&self, u: f32, v: f32, aspect: f32) -> [u8; 3] {
        let own = self.w as f32 / self.h as f32;
        let (mut u, mut v) = (u, v);
        if own > aspect {
            u = 0.5 + (u - 0.5) * aspect / own;
        } else {
            v = 0.5 + (v - 0.5) * own / aspect;
        }
        let x = ((u.clamp(0.0, 0.999)) * self.w as f32) as usize;
        let y = ((v.clamp(0.0, 0.999)) * self.h as f32) as usize;
        self.rgb[y * self.w + x]
    }
}

pub struct Found {
    /// Largest first.
    pub entries: Vec<Entry>,
    /// Pictures for the first [`SHOWN`] entries, filled in after.
    pub thumbs: Vec<OnceLock<Option<Thumb>>>,
}

/// Live state of a search, shared between the worker and the UI.
#[derive(Default)]
pub struct Search {
    /// Files compared so far, of how many need comparing.
    pub checked: AtomicU64,
    pub total: AtomicU64,
    /// Bytes in copies found so far.
    pub copies: AtomicU64,
    current: Mutex<String>,
    found: Mutex<Option<Arc<Found>>>,
}

impl Search {
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn found(&self) -> Option<Arc<Found>> {
        self.found.lock().unwrap().clone()
    }

    fn show(&self, path: &Path) {
        if let Ok(mut current) = self.current.try_lock() {
            current.clear();
            current.push_str(&path.to_string_lossy());
        }
    }
}

/// Where people keep their own files, most deliberate first: when copies
/// match, the one in the earliest folder stays.
fn roots(home: &Path) -> Vec<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "macos") {
        &["Pictures", "Documents", "Movies", "Desktop", "Downloads"]
    } else {
        &["Pictures", "Documents", "Videos", "Desktop", "Downloads"]
    };
    names
        .iter()
        .map(|n| home.join(n))
        .filter(|p| access::scannable(home, p))
        .collect()
}

/// Folders that belong to an app or a project, not to the person. Their
/// files go together; a copy there is not a spare.
fn skip_dir(name: &str) -> bool {
    const PACKAGES: [&str; 14] = [
        "app",
        "photoslibrary",
        "musiclibrary",
        "tvlibrary",
        "imovielibrary",
        "fcpbundle",
        "logicx",
        "band",
        "bundle",
        "framework",
        "xcodeproj",
        "xcworkspace",
        "sparsebundle",
        "lrlibrary",
    ];
    name.starts_with('.')
        || name == "node_modules"
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| PACKAGES.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Not on this Mac: iCloud keeps only a placeholder until it's opened, and
/// reading it would download it.
#[cfg(target_os = "macos")]
fn dataless(md: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    md.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn dataless(_md: &fs::Metadata) -> bool {
    false
}

fn is_screenshot(path: &Path, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let picture = [".png", ".jpg", ".jpeg", ".heic", ".mov", ".mp4"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if !picture {
        return false;
    }
    ["screenshot", "screen shot", "screen recording"]
        .iter()
        .any(|p| lower.starts_with(p))
        || marked_screenshot(path)
}

/// macOS tags every screenshot it saves, whatever the language or name.
#[cfg(target_os = "macos")]
fn marked_screenshot(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let name = c"com.apple.metadata:kMDItemIsScreenCapture";
    unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        ) >= 0
    }
}

#[cfg(not(target_os = "macos"))]
fn marked_screenshot(_path: &Path) -> bool {
    false
}

struct Seen {
    file: File,
    len: u64,
    id: (u64, u64),
    /// Index of the root it was found under, for which copy stays.
    root: usize,
}

#[derive(Default)]
struct Walked {
    big: Vec<Seen>,
    shots: Vec<File>,
}

fn walk<'s>(
    scope: &rayon::Scope<'s>,
    dir: PathBuf,
    root: usize,
    out: &'s Mutex<Walked>,
    search: &'s Search,
    now: SystemTime,
) {
    let Ok(read) = fs::read_dir(&dir) else {
        return;
    };
    let entries: Vec<_> = read.flatten().collect();
    // A folder under version control is a project, not loose files.
    if entries.iter().any(|e| e.file_name() == ".git") {
        return;
    }
    search.show(&dir);
    let mut big = Vec::new();
    let mut shots = Vec::new();
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if kind.is_dir() {
            if !skip_dir(&name) {
                scope.spawn(move |scope| walk(scope, path, root, out, search, now));
            }
            continue;
        }
        if !kind.is_file() || name.starts_with('.') {
            continue;
        }
        let Ok(md) = entry.metadata() else {
            continue;
        };
        if dataless(&md) {
            continue;
        }
        let file = File {
            path,
            size: md.blocks() * 512,
            modified: md.modified().unwrap_or(now),
        };
        let old = now.duration_since(file.modified).unwrap_or_default() > OLD;
        if old && is_screenshot(&file.path, &name) {
            shots.push(file.clone());
        }
        if md.len() >= MIN_COPY {
            big.push(Seen {
                file,
                len: md.len(),
                id: (md.dev(), md.ino()),
                root,
            });
        }
    }
    let mut out = out.lock().unwrap();
    out.big.extend(big);
    out.shots.extend(shots);
}

fn hash_edges(path: &Path, len: u64) -> io::Result<[u8; 32]> {
    let mut f = FsFile::open(path)?;
    let mut buf = vec![0; EDGE as usize];
    let mut hasher = blake3::Hasher::new();
    let n = f.read(&mut buf)?;
    hasher.update(&buf[..n]);
    if len > EDGE * 2 {
        f.seek(SeekFrom::Start(len - EDGE))?;
        f.read_exact(&mut buf)?;
        hasher.update(&buf);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn hash_all(path: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(FsFile::open(path)?)?;
    Ok(*hasher.finalize().as_bytes())
}

/// Keeps groups of two or more, by `key`.
fn regroup<K: std::hash::Hash + Eq + Send>(
    groups: Vec<Vec<Seen>>,
    search: &Search,
    key: impl Fn(&Seen) -> Option<K> + Sync,
) -> Vec<Vec<Seen>> {
    groups
        .into_par_iter()
        .flat_map_iter(|group| {
            let mut by: HashMap<K, Vec<Seen>> = HashMap::new();
            for seen in group {
                search.show(&seen.file.path);
                let k = key(&seen);
                search.checked.fetch_add(1, Relaxed);
                if let Some(k) = k {
                    by.entry(k).or_default().push(seen);
                }
            }
            by.into_values().filter(|g| g.len() > 1)
        })
        .collect()
}

/// Which copy stays: the one in the most deliberate folder, then the oldest
/// (the original), then the shortest name (not "photo (1).jpg").
fn keeper_order(a: &Seen, b: &Seen) -> std::cmp::Ordering {
    a.root
        .cmp(&b.root)
        .then(a.file.modified.cmp(&b.file.modified))
        .then(
            file_name(&a.file.path)
                .len()
                .cmp(&file_name(&b.file.path).len()),
        )
}

/// Bytes only this file holds: an APFS clone shares its blocks with the
/// file it was copied from, so trashing it frees nothing.
#[cfg(target_os = "macos")]
fn private_size(file: &File) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(file.path.as_os_str().as_bytes()) else {
        return file.size;
    };
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.forkattr = libc::ATTR_CMNEXT_PRIVATESIZE;
    let mut buf = [0u8; 16];
    let ok = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&raw mut list).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW,
        )
    } == 0;
    if !ok {
        return file.size;
    }
    // A u32 length, then the size.
    u64::from_ne_bytes(buf[4..12].try_into().unwrap())
}

#[cfg(not(target_os = "macos"))]
fn private_size(file: &File) -> u64 {
    file.size
}

fn search_now(search: &Search) -> Found {
    let home = disk::home();
    let now = SystemTime::now();
    let walked = Mutex::new(Walked::default());
    let roots = roots(&home);
    tree::pool().scope(|scope| {
        for (i, root) in roots.into_iter().enumerate() {
            let (walked, search) = (&walked, search);
            scope.spawn(move |scope| walk(scope, root, i, walked, search, now));
        }
    });
    let Walked { big, mut shots } = walked.into_inner().unwrap();

    // Same length first, then the same first and last bytes, then the same
    // everything: most files drop out before being read in full.
    let mut by_len: HashMap<u64, Vec<Seen>> = HashMap::new();
    for seen in big {
        by_len.entry(seen.len).or_default().push(seen);
    }
    let groups: Vec<Vec<Seen>> = by_len
        .into_values()
        .filter_map(|mut g| {
            // Hard links are one file under two names.
            g.sort_by_key(|s| s.id);
            g.dedup_by_key(|s| s.id);
            (g.len() > 1).then_some(g)
        })
        .collect();
    let edges: u64 = groups.iter().map(|g| g.len() as u64).sum();
    search.total.store(edges * 2, Relaxed);
    let groups =
        tree::pool().install(|| regroup(groups, search, |s| hash_edges(&s.file.path, s.len).ok()));
    // Files already ruled out need no second read.
    let left: u64 = groups.iter().map(|g| g.len() as u64).sum();
    search.total.store(edges + left, Relaxed);
    let groups = tree::pool().install(|| {
        regroup(groups, search, |s| {
            let hash = hash_all(&s.file.path).ok();
            if hash.is_some() {
                search.copies.fetch_add(s.file.size, Relaxed);
            }
            hash
        })
    });

    let mut entries: Vec<Entry> = groups
        .into_iter()
        .filter_map(|mut g| {
            g.sort_by(keeper_order);
            let files: Vec<File> = g.into_iter().map(|s| s.file).collect();
            let bytes: u64 = files[1..].iter().map(private_size).sum();
            (bytes >= MIN_COPY).then_some(Entry {
                kind: Kind::Copies(files),
                bytes,
            })
        })
        .collect();
    // A screenshot that is also a spare copy shows once, as the copy.
    let copies: std::collections::HashSet<PathBuf> = entries
        .iter()
        .flat_map(|e| e.files().iter().map(|f| f.path.clone()))
        .collect();
    shots.retain(|f| !copies.contains(&f.path));
    entries.extend(shots.into_iter().map(|file| Entry {
        bytes: file.size,
        kind: Kind::Screenshot(file),
    }));
    entries.sort_by_key(|e| std::cmp::Reverse(e.bytes));
    let thumbs = (0..entries.len().min(SHOWN))
        .map(|_| OnceLock::new())
        .collect();
    Found { entries, thumbs }
}

/// Searches in the background. Returns immediately.
pub fn find(search: Arc<Search>) {
    thread::spawn(move || {
        let found = Arc::new(search_now(&search));
        *search.found.lock().unwrap() = Some(found.clone());
        // Pictures come after, so the tiles show up first and fill in.
        tree::pool().install(|| {
            found.thumbs.par_iter().enumerate().for_each(|(i, slot)| {
                let _ = slot.set(thumb(&found.entries[i].files()[0].path));
            });
        });
    });
}

/// Sample data for `--demo`: nothing on disk is read.
pub fn demo(search: Arc<Search>) {
    thread::spawn(move || {
        let now = SystemTime::now();
        let total = 240;
        search.total.store(total, Relaxed);
        for i in 0..total {
            search.checked.store(i + 1, Relaxed);
            search.copies.store(i * 14_000_000, Relaxed);
            *search.current.lock().unwrap() = format!("~/Pictures/Trip/IMG_{:04}.HEIC", 2000 + i);
            thread::sleep(Duration::from_millis(12));
        }
        let file = |path: &str, mb: u64, days: u64| File {
            path: disk::home().join(path),
            size: mb * 1_000_000,
            modified: now - Duration::from_secs(days * 24 * 3600),
        };
        let mut entries = vec![
            Entry {
                kind: Kind::Copies(vec![
                    file("Movies/Demo final.mov", 1100, 300),
                    file("Desktop/Demo final copy.mov", 1100, 20),
                ]),
                bytes: 1_100_000_000,
            },
            Entry {
                kind: Kind::Copies(vec![
                    file("Pictures/Trip 2024/IMG_2041.HEIC", 412, 400),
                    file("Downloads/IMG_2041 (1).HEIC", 412, 30),
                ]),
                bytes: 412_000_000,
            },
            Entry {
                kind: Kind::Copies(vec![
                    file("Documents/Lease scan.pdf", 38, 500),
                    file("Downloads/Lease scan.pdf", 38, 90),
                    file("Desktop/Lease scan.pdf", 38, 10),
                ]),
                bytes: 76_000_000,
            },
        ];
        for i in 0..12 {
            let f = file(
                &format!(
                    "Desktop/Screenshot 2025-0{}-1{} at 10.12.png",
                    1 + i % 9,
                    i % 10
                ),
                3 + i * 2,
                120 + i * 9,
            );
            entries.push(Entry {
                bytes: f.size,
                kind: Kind::Screenshot(f),
            });
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.bytes));
        let thumbs = (0..entries.len().min(SHOWN))
            .map(|i| OnceLock::from(Some(sample_picture(i))))
            .collect();
        *search.found.lock().unwrap() = Some(Arc::new(Found { entries, thumbs }));
    });
}

/// A made-up picture for demo mode: a sunset over water for the first few,
/// a light window on a dark desktop for the screenshots.
fn sample_picture(i: usize) -> Thumb {
    let (w, h) = (64, 48);
    let mut rgb = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let px = if i < 3 {
                let sun = ((u - 0.3 - 0.2 * i as f32).powi(2) + (v - 0.45).powi(2)).sqrt();
                if v < 0.55 {
                    let glow = (1.0 - sun * 3.0).max(0.0);
                    [
                        (230.0 - 90.0 * v + 25.0 * glow) as u8,
                        (120.0 + 80.0 * glow) as u8,
                        (90.0 + 120.0 * v) as u8,
                    ]
                } else {
                    let ripple = ((y * 7 + x) % 9) as f32 / 9.0;
                    [
                        (40.0 + 60.0 * ripple) as u8,
                        (60.0 + 40.0 * ripple) as u8,
                        (110.0 + 50.0 * ripple) as u8,
                    ]
                }
            } else {
                let window = u > 0.12 && u < 0.88 && v > 0.15 && v < 0.85;
                let bar = window && v < 0.24;
                let line = window
                    && !bar
                    && (y % 5 == 0)
                    && u < 0.3 + 0.5 * ((y * 13 + i) % 7) as f32 / 7.0;
                if bar {
                    [210, 210, 214]
                } else if line {
                    [90, 90, 100]
                } else if window {
                    [245, 245, 247]
                } else {
                    [(30 + i * 9 % 60) as u8, 50, (110 + i * 13 % 80) as u8]
                }
            };
            rgb.push(px);
        }
    }
    Thumb { w, h, rgb }
}

/// Moves `paths` to the Trash. Returns, for each one moved, where it went.
pub fn trash(paths: Vec<PathBuf>) -> Vec<(PathBuf, PathBuf)> {
    paths
        .into_iter()
        .filter_map(|path| trash_one(&path).map(|to| (path, to)))
        .collect()
}

/// Moves trashed files back where they were, unless something new is there.
pub fn put_back(moved: &[(PathBuf, PathBuf)]) {
    for (from, to) in moved {
        if fs::symlink_metadata(from).is_err() {
            let _ = fs::rename(to, from);
        }
    }
    #[cfg(not(target_os = "macos"))]
    for (_, to) in moved {
        let _ = fs::remove_file(linux::info_path(to));
    }
}

#[cfg(target_os = "macos")]
fn trash_one(path: &Path) -> Option<PathBuf> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    let mut moved = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut moved))
        .ok()?;
    Some(PathBuf::from(moved?.path()?.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn trash_one(path: &Path) -> Option<PathBuf> {
    linux::trash(path)
}

/// The freedesktop.org Trash: the file goes in `files/`, and a note of where
/// it came from in `info/`, so file managers can put it back too.
#[cfg(not(target_os = "macos"))]
mod linux {
    use super::*;

    fn trash_dir() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| disk::home().join(".local/share"))
            .join("Trash")
    }

    pub fn info_path(trashed: &Path) -> PathBuf {
        let name = format!("{}.trashinfo", file_name(trashed));
        trash_dir().join("info").join(name)
    }

    pub fn trash(path: &Path) -> Option<PathBuf> {
        let dir = trash_dir();
        fs::create_dir_all(dir.join("files")).ok()?;
        fs::create_dir_all(dir.join("info")).ok()?;
        let name = file_name(path);
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (name.clone(), String::new()),
        };
        let to = (0..1000)
            .map(|n| match n {
                0 => dir.join("files").join(&name),
                n => dir.join("files").join(format!("{stem} {n}{ext}")),
            })
            .find(|p| fs::symlink_metadata(p).is_err())?;
        let info = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            path.display(),
            date_now()
        );
        fs::write(info_path(&to), info).ok()?;
        if fs::rename(path, &to).is_err() {
            let _ = fs::remove_file(info_path(&to));
            return None;
        }
        Some(to)
    }

    /// Local time as YYYY-MM-DDThh:mm:ss.
    fn date_now() -> String {
        let t = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

/// A picture of the file: the image itself when macOS can open it (photos,
/// screenshots, PDFs), else its Finder icon.
#[cfg(target_os = "macos")]
fn thumb(path: &Path) -> Option<Thumb> {
    const THUMB: f64 = 96.0;
    use objc2::AnyThread;
    use objc2_app_kit::{NSImage, NSWorkspace};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    let name = NSString::from_str(path.to_str()?);
    let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &name)
        .filter(|i| i.size().width > 0.0)
        .unwrap_or_else(|| NSWorkspace::sharedWorkspace().iconForFile(&name));
    let mut rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(THUMB * 2.0, THUMB * 2.0),
    );
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let (iw, ih) = (
        CGImage::width(Some(&cg)) as f64,
        CGImage::height(Some(&cg)) as f64,
    );
    if iw < 1.0 || ih < 1.0 {
        return None;
    }
    let scale = THUMB / iw.max(ih);
    let (w, h) = (
        ((iw * scale) as usize).max(1),
        ((ih * scale) as usize).max(1),
    );
    let mut buf = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb()?;
    let ctx = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    let full = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&ctx), full, Some(&cg));
    drop(ctx);
    // The bitmap's first row is the picture's top.
    // Colors are premultiplied, so see-through parts (an icon's edges) go
    // over the tile's own background instead of turning black.
    let over = |c: u8, a: u8, bg: u16| (c as u16 + bg * (255 - a as u16) / 255).min(255) as u8;
    let rgb = buf
        .chunks_exact(4)
        .map(|p| {
            [
                over(p[0], p[3], 0x1b),
                over(p[1], p[3], 0x1b),
                over(p[2], p[3], 0x1e),
            ]
        })
        .collect();
    Some(Thumb { w, h, rgb })
}

#[cfg(not(target_os = "macos"))]
fn thumb(_path: &Path) -> Option<Thumb> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_and_project_folders_are_skipped() {
        assert!(skip_dir("Photos Library.photoslibrary"));
        assert!(skip_dir("Xcode.app"));
        assert!(skip_dir("node_modules"));
        assert!(skip_dir(".cache"));
        assert!(!skip_dir("Trip 2024"));
        assert!(!skip_dir("v1.2"));
    }

    #[test]
    fn finds_copies_but_not_hard_links_or_lookalikes() {
        let dir = std::env::temp_dir().join(format!("bd-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        let data: Vec<u8> = (0..2_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut other = data.clone();
        other[1_000_000] ^= 1; // same size and edges, different middle
        fs::write(dir.join("a/photo.jpg"), &data).unwrap();
        fs::write(dir.join("b/photo (1).jpg"), &data).unwrap();
        fs::write(dir.join("b/other.jpg"), &other).unwrap();
        fs::hard_link(dir.join("a/photo.jpg"), dir.join("a/link.jpg")).unwrap();

        let walked = Mutex::new(Walked::default());
        let search = Search::default();
        let now = SystemTime::now();
        rayon::scope(|s| walk(s, dir.clone(), 0, &walked, &search, now));
        let big = walked.into_inner().unwrap().big;
        let mut g = vec![big];
        g[0].sort_by_key(|s| s.id);
        g[0].dedup_by_key(|s| s.id);
        assert_eq!(g[0].len(), 3, "the hard link is the same file");
        let g = regroup(g, &search, |s| hash_edges(&s.file.path, s.len).ok());
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].len(), 3, "edges can't tell the lookalike apart");
        let g = regroup(g, &search, |s| hash_all(&s.file.path).ok());
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
