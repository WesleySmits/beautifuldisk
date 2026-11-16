//! Developer clutter: caches, build output, agent worktrees, simulators and
//! old backups, found from the scanned tree and cleared on request.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, SystemTime};

use dither_gpui::dither_core::Tone;
use gpui_kit::Image;
use rayon::prelude::*;

use crate::disk::{Category, Progress};
use crate::icons::{self, Logo, Source};
use crate::tree::{self, Index};

/// Smaller finds are not worth a chip.
const MIN_BYTES: u64 = 50_000_000;
/// A worktree touched more recently than this may be in use by an agent.
const RECENT: Duration = Duration::from_secs(2 * 24 * 3600);

pub struct Item {
    pub name: String,
    /// Shown on hover: what it is, or what is kept.
    pub detail: String,
    pub tone: Tone,
    pub icon: Option<Arc<Image>>,
    pub category: Category,
    /// Selected when first shown. Off for things that may hold your own work.
    pub on: bool,
    left: AtomicU64,
    how: How,
}

enum How {
    /// Empty these folders, keeping the folders. Skips the listed children.
    Contents(Vec<PathBuf>, Vec<PathBuf>),
    /// Delete these folders or files outright.
    Paths(Vec<(PathBuf, u64)>),
    /// Git worktrees, removed with `git worktree remove`.
    Worktrees(Vec<(PathBuf, u64)>),
    /// Simulator device folders, deleted with `simctl`.
    Simulators(Vec<(PathBuf, u64)>),
    /// Android virtual devices: the .avd folder and its .ini.
    Emulators(Vec<(PathBuf, u64)>),
    /// Sample data for `--demo`.
    Demo,
}

impl Item {
    pub fn left(&self) -> u64 {
        self.left.load(Relaxed)
    }
}

struct Spec {
    name: &'static str,
    detail: &'static str,
    /// Relative to home.
    paths: &'static [&'static str],
    hue: f32,
    icon: Source,
    on: bool,
}

const fn spec(
    name: &'static str,
    detail: &'static str,
    paths: &'static [&'static str],
    hue: f32,
    icon: Source,
    on: bool,
) -> Spec {
    Spec {
        name,
        detail,
        paths,
        hue,
        icon,
        on,
    }
}

const XCODE: Source = Source::App("com.apple.dt.Xcode", None);
const SIMULATOR: Source = Source::App("com.apple.iphonesimulator", Some(Logo::Apple));
const AGAIN: &str = "Downloaded again when needed.";
const REBUILT: &str = "Rebuilt on the next build.";

/// Folders whose contents can go, in no particular order.
const CACHES: &[Spec] = &[
    spec(
        "Xcode build data",
        REBUILT,
        &["Library/Developer/Xcode/DerivedData"],
        215.0,
        XCODE,
        true,
    ),
    spec(
        "Xcode caches",
        AGAIN,
        &["Library/Caches/com.apple.dt.Xcode"],
        215.0,
        XCODE,
        true,
    ),
    spec(
        "Simulator caches",
        REBUILT,
        &["Library/Developer/CoreSimulator/Caches"],
        220.0,
        SIMULATOR,
        true,
    ),
    spec(
        "Device support",
        "Debug symbols for connected iPhones. Copied again on the next connect.",
        &[
            "Library/Developer/Xcode/iOS DeviceSupport",
            "Library/Developer/Xcode/watchOS DeviceSupport",
            "Library/Developer/Xcode/tvOS DeviceSupport",
            "Library/Developer/Xcode/visionOS DeviceSupport",
        ],
        210.0,
        XCODE,
        false,
    ),
    spec(
        "Xcode archives",
        "Builds you archived for release.",
        &["Library/Developer/Xcode/Archives"],
        205.0,
        XCODE,
        false,
    ),
    spec(
        "Gradle",
        AGAIN,
        &[".gradle/caches", ".gradle/wrapper/dists"],
        195.0,
        Source::Logo(Logo::Gradle),
        true,
    ),
    spec(
        "pnpm store",
        AGAIN,
        &[
            "Library/pnpm/store",
            ".local/share/pnpm/store",
            "Library/Caches/pnpm",
        ],
        45.0,
        Source::Logo(Logo::Pnpm),
        true,
    ),
    spec(
        "bun cache",
        AGAIN,
        &[".bun/install/cache"],
        70.0,
        Source::Logo(Logo::Bun),
        true,
    ),
    spec(
        "npm cache",
        AGAIN,
        &[".npm/_cacache"],
        25.0,
        Source::Logo(Logo::Npm),
        true,
    ),
    spec(
        "Yarn cache",
        AGAIN,
        &["Library/Caches/Yarn", ".yarn/berry/cache"],
        200.0,
        Source::Logo(Logo::Yarn),
        true,
    ),
    spec(
        "Go modules",
        AGAIN,
        &["go/pkg/mod", "Library/Caches/go-build"],
        190.0,
        Source::Logo(Logo::Go),
        true,
    ),
    spec(
        "Cargo downloads",
        AGAIN,
        &[
            ".cargo/registry/cache",
            ".cargo/registry/src",
            ".cargo/git/checkouts",
        ],
        40.0,
        Source::Logo(Logo::Rust),
        true,
    ),
    spec(
        "uv cache",
        AGAIN,
        &[".cache/uv"],
        300.0,
        Source::Logo(Logo::Uv),
        true,
    ),
    spec(
        "pip cache",
        AGAIN,
        &["Library/Caches/pip"],
        55.0,
        Source::Logo(Logo::Python),
        true,
    ),
    spec(
        "Homebrew downloads",
        AGAIN,
        &["Library/Caches/Homebrew"],
        50.0,
        Source::Logo(Logo::HomeBrew),
        true,
    ),
    spec(
        "CocoaPods",
        AGAIN,
        &["Library/Caches/CocoaPods"],
        15.0,
        Source::Logo(Logo::CocoaPods),
        true,
    ),
    spec(
        "Playwright browsers",
        AGAIN,
        &["Library/Caches/ms-playwright"],
        140.0,
        Source::File,
        true,
    ),
    spec(
        "Chrome cache",
        AGAIN,
        &["Library/Caches/Google/Chrome"],
        220.0,
        Source::App("com.google.Chrome", None),
        true,
    ),
    spec(
        "Arc cache",
        AGAIN,
        &["Library/Caches/company.thebrowser.Browser"],
        330.0,
        Source::App("company.thebrowser.Browser", None),
        true,
    ),
    spec(
        "Brave cache",
        AGAIN,
        &["Library/Caches/BraveSoftware"],
        20.0,
        Source::App("com.brave.Browser", None),
        true,
    ),
    spec(
        "Firefox cache",
        AGAIN,
        &["Library/Caches/Firefox"],
        30.0,
        Source::App("org.mozilla.firefox", None),
        true,
    ),
    spec(
        "Safari cache",
        AGAIN,
        &["Library/Caches/com.apple.Safari"],
        205.0,
        Source::App("com.apple.Safari", None),
        true,
    ),
    spec(
        "Hugging Face models",
        "Models you downloaded.",
        &[".cache/huggingface"],
        50.0,
        Source::Logo(Logo::HuggingFace),
        false,
    ),
    spec(
        "Ollama models",
        "Models you downloaded.",
        &[".ollama/models"],
        0.0,
        Source::App("com.electron.ollama", Some(Logo::Ollama)),
        false,
    ),
    spec(
        "LM Studio models",
        "Models you downloaded.",
        &[".lmstudio/models"],
        260.0,
        Source::App("ai.elementlabs.lmstudio", None),
        false,
    ),
    spec(
        "Trash",
        "Things you already deleted.",
        &[".Trash"],
        0.0,
        Source::File,
        true,
    ),
];

/// Where coding agents keep their worktrees.
const AGENTS: &[(&str, &str, Source)] = &[
    (
        "T3 Code",
        ".t3/worktrees",
        Source::App("com.t3tools.t3code", Some(Logo::Git)),
    ),
    (
        "Codex",
        ".codex/worktrees",
        Source::App("com.openai.codex", Some(Logo::OpenAi)),
    ),
    (
        "Cursor",
        ".cursor/worktrees",
        Source::App("com.todesktop.230313mzl4w4u92", Some(Logo::Git)),
    ),
    (
        "Conductor",
        "conductor/workspaces",
        Source::App("com.conductor.app", Some(Logo::Git)),
    ),
    ("Capy", ".capy/worktrees", Source::Logo(Logo::Git)),
    ("Verdent", ".verdent/worktrees", Source::Logo(Logo::Git)),
];

/// Where people keep projects: searched for node_modules, build output,
/// Claude Code worktrees and stray backups.
const PROJECTS: &[&str] = &["Developer", "Projects", "Documents", "Desktop", "Downloads"];

/// Folders the tree must cover for detection, beyond the category roots.
pub const EXTRA_ROOTS: &[&str] = &[
    ".t3",
    ".codex",
    ".claude",
    ".cursor",
    "conductor",
    ".capy",
    ".verdent",
    ".android",
    "go",
    ".cache",
    ".ollama",
    ".lmstudio",
    ".yarn",
    ".local/share/pnpm",
];

fn category_of(home: &Path, path: &Path) -> Category {
    if path.starts_with(home.join("Library/Caches")) {
        Category::Caches
    } else if path.starts_with(home.join(".Trash")) {
        Category::Trash
    } else {
        Category::Developer
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

struct Found {
    name: String,
    detail: String,
    hue: f32,
    icon: Source,
    /// Gives `Source::File` something to show.
    icon_file: Option<PathBuf>,
    category: Category,
    on: bool,
    bytes: u64,
    how: How,
}

/// Everything worth offering, largest first.
pub fn detect(index: &Index, home: &Path) -> Vec<Item> {
    let mut found = Vec::new();
    let mut claimed = Vec::new();

    for spec in CACHES {
        let paths: Vec<PathBuf> = spec.paths.iter().map(|p| home.join(p)).collect();
        let bytes = paths.iter().map(|p| index.size(p)).sum();
        claimed.extend(paths.iter().cloned());
        found.push(Found {
            name: spec.name.into(),
            detail: spec.detail.into(),
            hue: spec.hue,
            icon: spec.icon,
            icon_file: paths.first().cloned(),
            category: category_of(home, &paths[0]),
            on: spec.on,
            bytes,
            how: How::Contents(paths, Vec::new()),
        });
    }

    // Everything else in ~/Library/Caches, minus what the specs above cover.
    let caches = home.join("Library/Caches");
    let skip: Vec<_> = index
        .subdirs(&caches)
        .into_iter()
        .filter(|(child, _)| claimed.iter().any(|c| c.starts_with(child)))
        .collect();
    found.push(Found {
        name: "App caches".into(),
        detail: "Apps rebuild these when they need them.".into(),
        hue: 30.0,
        icon: Source::File,
        icon_file: Some(caches.clone()),
        category: Category::Caches,
        on: true,
        bytes: index
            .size(&caches)
            .saturating_sub(skip.iter().map(|(_, b)| b).sum()),
        how: How::Contents(vec![caches], skip.into_iter().map(|(p, _)| p).collect()),
    });

    let projects: Vec<PathBuf> = PROJECTS.iter().map(|p| home.join(p)).collect();

    let mut worktrees = Vec::new();
    for (tool, dir, icon) in AGENTS {
        worktrees.push((
            tool.to_string(),
            *icon,
            find_worktrees(index, &home.join(dir)),
        ));
    }
    let claude: Vec<_> = index
        .search(&projects, |name, parent, _| {
            name == b"worktrees" && parent == b".claude"
        })
        .into_iter()
        .flat_map(|(dir, _)| find_worktrees(index, &dir))
        .collect();
    worktrees.push((
        "Claude Code".into(),
        Source::App("com.anthropic.claudefordesktop", Some(Logo::Claude)),
        claude,
    ));
    let all_worktrees: Vec<PathBuf> = worktrees
        .iter()
        .flat_map(|(_, _, w)| w.iter().map(|(p, _)| p.clone()))
        .collect();
    for (tool, icon, list) in worktrees {
        if list.is_empty() {
            continue;
        }
        let total = list.len();
        let (keep, clear): (Vec<_>, Vec<_>) = list.into_par_iter().partition(|(p, _)| in_use(p));
        let detail = if keep.is_empty() {
            count(total, "worktree", "worktrees")
        } else {
            format!(
                "{}. Keeps {} with changes or used in the last 2 days.",
                count(total, "worktree", "worktrees"),
                keep.len()
            )
        };
        found.push(Found {
            name: format!("{tool} worktrees"),
            detail,
            hue: 15.0,
            icon,
            icon_file: None,
            category: Category::Developer,
            on: true,
            bytes: clear.iter().map(|(_, b)| b).sum(),
            how: How::Worktrees(clear),
        });
    }

    let t3_backups = index.subdirs(&home.join(".t3/backups"));
    found.push(Found {
        name: "T3 Code backups".into(),
        detail: count(t3_backups.len(), "backup", "backups"),
        hue: 280.0,
        icon: Source::App("com.t3tools.t3code", Some(Logo::Git)),
        icon_file: None,
        category: Category::Developer,
        on: false,
        bytes: t3_backups.iter().map(|(_, b)| b).sum(),
        how: How::Paths(t3_backups),
    });

    let outside_worktrees = |list: Vec<(PathBuf, u64)>| -> Vec<(PathBuf, u64)> {
        list.into_iter()
            .filter(|(p, _)| !all_worktrees.iter().any(|w| p.starts_with(w)))
            .collect()
    };
    let modules = outside_worktrees(index.search(&projects, |name, _, _| name == b"node_modules"));
    found.push(Found {
        name: "node_modules".into(),
        detail: format!(
            "{}. Reinstalled with your package manager.",
            count(modules.len(), "folder", "folders")
        ),
        hue: 100.0,
        icon: Source::Logo(Logo::Node),
        icon_file: None,
        category: Category::Developer,
        on: false,
        bytes: modules.iter().map(|(_, b)| b).sum(),
        how: How::Paths(modules),
    });
    let targets =
        outside_worktrees(index.search(&projects, |name, _, build| build && name == b"target"));
    found.push(Found {
        name: "Rust build output".into(),
        detail: format!(
            "{}. {REBUILT}",
            count(targets.len(), "target folder", "target folders")
        ),
        hue: 35.0,
        icon: Source::Logo(Logo::Rust),
        icon_file: None,
        category: Category::Developer,
        on: true,
        bytes: targets.iter().map(|(_, b)| b).sum(),
        how: How::Paths(targets),
    });

    let devices: Vec<_> = index
        .subdirs(&home.join("Library/Developer/CoreSimulator/Devices"))
        .into_iter()
        .filter(|(p, _)| p.file_name().is_some_and(|n| n.len() == 36))
        .collect();
    found.push(Found {
        name: "iOS simulators".into(),
        detail: format!(
            "{}. Running ones are kept.",
            count(devices.len(), "device", "devices")
        ),
        hue: 225.0,
        icon: SIMULATOR,
        icon_file: None,
        category: Category::Developer,
        on: true,
        bytes: devices.iter().map(|(_, b)| b).sum(),
        how: How::Simulators(devices),
    });

    let avds: Vec<_> = index
        .subdirs(&home.join(".android/avd"))
        .into_iter()
        .filter(|(p, _)| p.extension().is_some_and(|e| e == "avd"))
        .collect();
    found.push(Found {
        name: "Android emulators".into(),
        detail: format!(
            "{}. Running ones are kept.",
            count(avds.len(), "device", "devices")
        ),
        hue: 145.0,
        icon: Source::App("com.google.android.studio", Some(Logo::Android)),
        icon_file: None,
        category: Category::Developer,
        on: false,
        bytes: avds.iter().map(|(_, b)| b).sum(),
        how: How::Emulators(avds),
    });

    let backups = outside_worktrees(index.search(&projects, |name, _, _| is_backup(name)))
        .into_iter()
        .filter(|(_, b)| *b >= MIN_BYTES)
        .collect::<Vec<_>>();
    let names: Vec<String> = backups
        .iter()
        .take(3)
        .filter_map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    found.push(Found {
        name: "Old backups".into(),
        detail: format!(
            "{}: {}.",
            count(backups.len(), "folder", "folders"),
            names.join(", ")
        ),
        hue: 265.0,
        icon: Source::App("com.apple.backup.launcher", None),
        icon_file: None,
        category: Category::Documents,
        on: false,
        bytes: backups.iter().map(|(_, b)| b).sum(),
        how: How::Paths(backups),
    });

    let installers = installers(&home.join("Downloads"));
    found.push(Found {
        name: "Installers".into(),
        detail: format!(
            "{} in Downloads.",
            count(
                installers.len(),
                "disk image or package",
                "disk images and packages"
            )
        ),
        hue: 185.0,
        icon: Source::File,
        icon_file: installers.first().map(|(p, _)| p.clone()),
        category: Category::Documents,
        on: false,
        bytes: installers.iter().map(|(_, b)| b).sum(),
        how: How::Paths(installers),
    });

    found.retain(|f| f.bytes >= MIN_BYTES);
    found.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    found
        .into_iter()
        .map(|f| Item {
            icon: icons::load(f.icon, f.icon_file.as_deref()),
            name: f.name,
            detail: f.detail,
            tone: Tone::Hue(f.hue),
            category: f.category,
            on: f.on,
            left: AtomicU64::new(f.bytes),
            how: f.how,
        })
        .collect()
}

fn is_backup(name: &[u8]) -> bool {
    let name = String::from_utf8_lossy(name).to_lowercase();
    name.starts_with("backup")
        || name.ends_with("backup")
        || name.ends_with("backups")
        || name.ends_with(".bak")
        || name.ends_with(".old")
}

/// Git worktrees up to three levels below `dir`.
fn find_worktrees(index: &Index, dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for parent in level {
            for (path, bytes) in index.subdirs(&parent) {
                // A worktree's .git is a file pointing at the main repository.
                if path.join(".git").is_file() {
                    found.push((path, bytes));
                } else {
                    next.push(path);
                }
            }
        }
        level = next;
    }
    found
}

/// Whether a worktree has uncommitted work or was used recently. Errs on
/// the side of keeping it.
fn in_use(worktree: &Path) -> bool {
    let Ok(link) = fs::read_to_string(worktree.join(".git")) else {
        return true;
    };
    let Some(gitdir) = link.trim().strip_prefix("gitdir: ") else {
        return true;
    };
    let recent = ["index", "HEAD", "logs/HEAD"].iter().any(|f| {
        fs::metadata(Path::new(gitdir).join(f))
            .and_then(|m| m.modified())
            .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < RECENT)
    });
    if recent {
        return true;
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain"])
        .output();
    !matches!(status, Ok(out) if out.status.success() && out.stdout.is_empty())
}

fn installers(downloads: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(downloads) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            [".dmg", ".pkg", ".iso", ".xip", ".ipsw"]
                .iter()
                .any(|ext| name.ends_with(ext))
        })
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file().then(|| (e.path(), md.blocks() * 512))
        })
        .collect()
}

/// Clears all `chosen` items at once, one thread each, sharing the pool.
pub fn clean(items: Arc<Vec<Item>>, chosen: Vec<usize>, progress: Arc<Progress>, demo: bool) {
    progress.start_cleaning();
    std::thread::spawn(move || {
        std::thread::scope(|scope| {
            for i in chosen {
                let (item, progress) = (&items[i], &*progress);
                scope.spawn(move || {
                    progress.set_cleaning(i, true);
                    let freed = |bytes: u64| {
                        let _ = item
                            .left
                            .fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(bytes)));
                        progress.remove(item.category, bytes);
                    };
                    if demo {
                        demo_clean(item, &freed);
                    } else {
                        tree::pool().install(|| run(&item.how, progress, &freed));
                    }
                    progress.set_cleaning(i, false);
                });
            }
        });
        progress.finish_cleaning();
    });
}

fn run(how: &How, progress: &Progress, freed: &(dyn Fn(u64) + Sync)) {
    match how {
        How::Contents(dirs, skip) => {
            for dir in dirs {
                let Ok(entries) = fs::read_dir(dir) else {
                    continue;
                };
                let children: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| !skip.contains(p))
                    .collect();
                children
                    .par_iter()
                    .for_each(|p| remove_all(p, progress, freed));
            }
        }
        How::Paths(paths) => {
            paths
                .par_iter()
                .for_each(|(p, _)| remove_all(p, progress, freed));
        }
        How::Worktrees(list) => {
            list.par_iter().for_each(|(path, bytes)| {
                progress.show(path);
                // Without --force, git refuses worktrees with changes.
                let removed = Command::new("git")
                    .arg("-C")
                    .arg(path)
                    .args(["worktree", "remove"])
                    .arg(path)
                    .output()
                    .is_ok_and(|o| o.status.success());
                if removed {
                    freed(*bytes);
                }
            });
        }
        How::Simulators(devices) => {
            let booted = Command::new("xcrun")
                .args(["simctl", "list", "devices", "booted"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            for (path, bytes) in devices {
                let Some(udid) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if booted.contains(udid) {
                    continue;
                }
                progress.show(path);
                let deleted = Command::new("xcrun")
                    .args(["simctl", "delete", udid])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if deleted {
                    freed(*bytes);
                }
            }
        }
        How::Emulators(avds) => {
            for (avd, _) in avds {
                // The emulator holds .lock files inside a running device.
                let running = fs::read_dir(avd).is_ok_and(|mut entries| {
                    entries
                        .any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "lock")))
                });
                if running {
                    continue;
                }
                remove_all(avd, progress, freed);
                let _ = fs::remove_file(avd.with_extension("ini"));
            }
        }
        How::Demo => {}
    }
}

/// Deletes `path` and everything in it. Never follows symlinks.
fn remove_all(path: &Path, progress: &Progress, freed: &(dyn Fn(u64) + Sync)) {
    let Ok(md) = fs::symlink_metadata(path) else {
        return;
    };
    if !md.is_dir() {
        progress.show(path);
        if fs::remove_file(path).is_ok() {
            // Same share-per-link counting as the scan.
            freed(md.blocks() * 512 / md.nlink().max(1));
        }
        return;
    }
    // Go's module cache is read-only; entries can't be removed until the
    // folder is writable again.
    if md.mode() & 0o700 != 0o700 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(md.mode() | 0o700));
    }
    if let Ok(entries) = fs::read_dir(path) {
        let children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        children
            .par_iter()
            .for_each(|p| remove_all(p, progress, freed));
    }
    let _ = fs::remove_dir(path);
}

/// Sample items for `--demo`, sized like a busy developer Mac.
pub fn demo() -> Vec<Item> {
    let item = |name: &str, detail: &str, gb: f64, hue: f32, icon: Source, on: bool| Item {
        category: if name == "Chrome cache" {
            Category::Caches
        } else {
            Category::Developer
        },
        name: name.into(),
        detail: detail.into(),
        tone: Tone::Hue(hue),
        icon: icons::load(icon, None),
        on,
        left: AtomicU64::new((gb * 1e9) as u64),
        how: How::Demo,
    };
    vec![
        item(
            "iOS simulators",
            "21 devices. Running ones are kept.",
            48.5,
            225.0,
            SIMULATOR,
            true,
        ),
        item(
            "T3 Code worktrees",
            "13 worktrees",
            21.2,
            15.0,
            Source::App("com.t3tools.t3code", Some(Logo::Git)),
            true,
        ),
        item(
            "node_modules",
            "90 folders. Reinstalled with your package manager.",
            21.5,
            100.0,
            Source::Logo(Logo::Node),
            false,
        ),
        item(
            "Chrome cache",
            AGAIN,
            8.9,
            220.0,
            Source::App("com.google.Chrome", None),
            true,
        ),
        item("Xcode build data", REBUILT, 8.2, 215.0, XCODE, true),
        item(
            "pnpm store",
            AGAIN,
            6.2,
            45.0,
            Source::Logo(Logo::Pnpm),
            true,
        ),
        item(
            "Android emulators",
            "2 devices. Running ones are kept.",
            5.2,
            145.0,
            Source::Logo(Logo::Android),
            false,
        ),
        item(
            "Gradle",
            AGAIN,
            4.5,
            195.0,
            Source::Logo(Logo::Gradle),
            true,
        ),
        item(
            "Go modules",
            AGAIN,
            3.5,
            190.0,
            Source::Logo(Logo::Go),
            true,
        ),
        item(
            "Capy worktrees",
            "6 worktrees",
            2.5,
            15.0,
            Source::Logo(Logo::Git),
            true,
        ),
        item("bun cache", AGAIN, 2.0, 70.0, Source::Logo(Logo::Bun), true),
        item(
            "Old backups",
            "2 folders: site-backup, db.old.",
            1.8,
            265.0,
            Source::App("com.apple.backup.launcher", None),
            false,
        ),
        item(
            "CocoaPods",
            AGAIN,
            0.9,
            15.0,
            Source::Logo(Logo::CocoaPods),
            true,
        ),
        item("npm cache", AGAIN, 0.7, 25.0, Source::Logo(Logo::Npm), true),
    ]
}

fn demo_clean(item: &Item, freed: &dyn Fn(u64)) {
    let start = item.left();
    let steps = 40;
    for _ in 0..steps {
        freed(start / steps);
        std::thread::sleep(Duration::from_millis(18));
    }
    freed(item.left());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn items_clear_at_the_same_time() {
        let items = Arc::new(demo());
        let chosen: Vec<usize> = (0..items.len()).collect();
        let progress = Arc::new(Progress::default());
        let start = Instant::now();
        clean(items.clone(), chosen, progress.clone(), true);
        let mut most = 0;
        while !progress.is_done() {
            most = most.max(progress.cleaning().count_ones());
            std::thread::sleep(Duration::from_millis(5));
        }
        // One demo item takes about 0.72s; one after another would take many times that.
        assert!(start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
        assert!(most > 1);
        assert_eq!(progress.cleaning(), 0);
        assert!(items.iter().all(|i| i.left() == 0));
    }
}
