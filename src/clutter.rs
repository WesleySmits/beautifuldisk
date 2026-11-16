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

use crate::disk::{self, Category, Progress};
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
    /// A command for the `docker` binary at this path that frees about this
    /// many bytes.
    Docker(PathBuf, &'static [&'static str], u64),
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
            ".cache/pnpm",
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
        &["Library/Caches/Yarn", ".yarn/berry/cache", ".cache/yarn"],
        200.0,
        Source::Logo(Logo::Yarn),
        true,
    ),
    spec(
        "Go modules",
        AGAIN,
        &["go/pkg/mod", "Library/Caches/go-build", ".cache/go-build"],
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
        &["Library/Caches/pip", ".cache/pip"],
        55.0,
        Source::Logo(Logo::Python),
        true,
    ),
    spec(
        "Homebrew downloads",
        AGAIN,
        &["Library/Caches/Homebrew", ".cache/Homebrew"],
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
        &["Library/Caches/ms-playwright", ".cache/ms-playwright"],
        140.0,
        Source::File,
        true,
    ),
    spec(
        "Chrome cache",
        AGAIN,
        &[
            "Library/Caches/Google/Chrome",
            ".cache/google-chrome",
            ".cache/chromium",
        ],
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
        &["Library/Caches/BraveSoftware", ".cache/BraveSoftware"],
        20.0,
        Source::App("com.brave.Browser", None),
        true,
    ),
    spec(
        "Firefox cache",
        AGAIN,
        &["Library/Caches/Firefox", ".cache/mozilla"],
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
        "Maven",
        "Downloaded again when needed. Holds what you installed locally too.",
        &[".m2/repository"],
        10.0,
        Source::File,
        false,
    ),
    spec(
        "Deno cache",
        AGAIN,
        &["Library/Caches/deno", ".cache/deno"],
        170.0,
        Source::File,
        true,
    ),
    spec(
        "Composer cache",
        AGAIN,
        &[
            "Library/Caches/composer",
            ".cache/composer",
            ".composer/cache",
        ],
        240.0,
        Source::File,
        true,
    ),
    spec(
        "Dart and Flutter packages",
        AGAIN,
        &[".pub-cache"],
        200.0,
        Source::File,
        true,
    ),
    spec(
        "Bazel cache",
        REBUILT,
        &[".cache/bazel"],
        120.0,
        Source::File,
        true,
    ),
    spec(
        "Compiler caches",
        "ccache and sccache. Rebuilt on the next build.",
        &[
            ".ccache",
            ".cache/ccache",
            "Library/Caches/ccache",
            ".cache/sccache",
            "Library/Caches/Mozilla.sccache",
        ],
        30.0,
        Source::File,
        true,
    ),
    spec(
        "node-gyp headers",
        AGAIN,
        &[".cache/node-gyp", "Library/Caches/node-gyp"],
        95.0,
        Source::Logo(Logo::Node),
        true,
    ),
    spec(
        "Test browsers",
        "Cypress and Puppeteer. Downloaded again when needed.",
        &[
            ".cache/Cypress",
            "Library/Caches/Cypress",
            ".cache/puppeteer",
        ],
        150.0,
        Source::File,
        true,
    ),
    spec(
        "Electron downloads",
        AGAIN,
        &[
            ".cache/electron",
            "Library/Caches/electron",
            ".cache/electron-builder",
            "Library/Caches/electron-builder",
        ],
        200.0,
        Source::File,
        true,
    ),
    spec(
        "pre-commit hooks",
        AGAIN,
        &[".cache/pre-commit"],
        60.0,
        Source::Logo(Logo::Python),
        true,
    ),
    spec(
        "JetBrains caches",
        "Indexes and caches. Rebuilt when you open a project.",
        &[".cache/JetBrains", "Library/Caches/JetBrains"],
        320.0,
        Source::File,
        true,
    ),
    spec(
        "VS Code caches",
        AGAIN,
        &[
            "Library/Application Support/Code/CachedData",
            "Library/Application Support/Code/CachedExtensionVSIXs",
            ".config/Code/CachedData",
            ".config/Code/CachedExtensionVSIXs",
        ],
        210.0,
        Source::App("com.microsoft.VSCode", None),
        true,
    ),
    spec(
        "PyTorch models",
        "Models you downloaded.",
        &[".cache/torch"],
        15.0,
        Source::Logo(Logo::Python),
        false,
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
        &[
            ".Trash",
            ".local/share/Trash/files",
            ".local/share/Trash/info",
        ],
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
    ".pub-cache",
    ".composer",
    ".ccache",
    ".config/Code",
    "Library/Application Support/Code",
];

/// The category the scan counted `path` under: its deepest root's.
fn category_of(roots: &[(PathBuf, Category)], path: &Path) -> Category {
    roots
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map_or(Category::System, |(_, category)| *category)
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
    let roots = disk::roots(home);

    for spec in CACHES {
        let paths: Vec<PathBuf> = spec.paths.iter().map(|p| home.join(p)).collect();
        let bytes = paths.iter().map(|p| index.size(p)).sum();
        // Specs list paths for both systems; categorize by the one in use.
        let main = paths
            .iter()
            .find(|p| index.size(p) > 0)
            .unwrap_or(&paths[0]);
        let category = category_of(&roots, main);
        let icon_file = Some(main.clone());
        claimed.extend(paths.iter().cloned());
        found.push(Found {
            name: spec.name.into(),
            detail: spec.detail.into(),
            hue: spec.hue,
            icon: spec.icon,
            icon_file,
            category,
            on: spec.on,
            bytes,
            how: How::Contents(paths, Vec::new()),
        });
    }

    // Everything else in the user cache folder, minus what the specs above cover.
    let caches = home.join(if cfg!(target_os = "macos") {
        "Library/Caches"
    } else {
        ".cache"
    });
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
    let web = outside_worktrees(index.search(&projects, |name, _, _| {
        matches!(
            name,
            b".next" | b".turbo" | b".nuxt" | b".svelte-kit" | b".parcel-cache"
        )
    }));
    found.push(Found {
        name: "Web build caches".into(),
        detail: format!(
            "{}: .next, .turbo and similar. {REBUILT}",
            count(web.len(), "folder", "folders")
        ),
        hue: 250.0,
        icon: Source::Logo(Logo::Node),
        icon_file: None,
        category: Category::Developer,
        on: true,
        bytes: web.iter().map(|(_, b)| b).sum(),
        how: How::Paths(web),
    });
    let venvs = outside_worktrees(index.search(&projects, |name, _, _| name == b".venv"));
    found.push(Found {
        name: "Python environments".into(),
        detail: format!(
            "{}. Reinstalled from your project's requirements.",
            count(venvs.len(), ".venv folder", ".venv folders")
        ),
        hue: 55.0,
        icon: Source::Logo(Logo::Python),
        icon_file: None,
        category: Category::Developer,
        on: false,
        bytes: venvs.iter().map(|(_, b)| b).sum(),
        how: How::Paths(venvs),
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
            count(installers.len(), "installer", "installers")
        ),
        hue: 185.0,
        icon: Source::File,
        icon_file: installers.first().map(|(p, _)| p.clone()),
        category: Category::Documents,
        on: false,
        bytes: installers.iter().map(|(_, b)| b).sum(),
        how: How::Paths(installers),
    });

    found.extend(docker());

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
            [
                ".dmg",
                ".pkg",
                ".iso",
                ".xip",
                ".ipsw",
                ".deb",
                ".rpm",
                ".appimage",
            ]
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
                    } else if let Some(guard) = Guard::new(&disk::home()) {
                        let clear = Clear {
                            guard: &guard,
                            progress,
                            freed: &freed,
                        };
                        tree::pool().install(|| run(&item.how, &clear));
                    }
                    progress.set_cleaning(i, false);
                });
            }
        });
        progress.finish_cleaning();
    });
}

/// What clearing may touch. Every deletion goes through `check`, so a
/// wrong path, a symlink or a mounted disk can never reach system files.
struct Guard {
    home: PathBuf,
    uid: u32,
}

impl Guard {
    /// None when clearing would be unsafe: running as root, or no real home.
    fn new(home: &Path) -> Option<Guard> {
        let uid = unsafe { libc::geteuid() };
        if uid == 0 || !home.is_absolute() || home.parent().is_none() || !home.is_dir() {
            return None;
        }
        Some(Guard {
            home: home.to_path_buf(),
            uid,
        })
    }

    /// The disk `path` is on, if it may be deleted: below the home folder,
    /// reached without a symlinked folder on the way, and owned by the user.
    fn check(&self, path: &Path) -> Option<u64> {
        let below = path.strip_prefix(&self.home).ok()?;
        if below.as_os_str().is_empty() || below.components().any(|c| c.as_os_str() == "..") {
            return None;
        }
        let mut folder = self.home.clone();
        let mut parts = below.components().peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            folder.push(part);
            if fs::symlink_metadata(&folder).ok()?.file_type().is_symlink() {
                return None;
            }
        }
        let md = fs::symlink_metadata(path).ok()?;
        (md.uid() == self.uid).then(|| md.dev())
    }
}

/// Everything a clearing worker needs.
struct Clear<'a> {
    guard: &'a Guard,
    progress: &'a Progress,
    freed: &'a (dyn Fn(u64) + Sync),
}

impl Clear<'_> {
    /// Deletes `path` if the guard allows it.
    fn remove(&self, path: &Path) {
        if let Some(dev) = self.guard.check(path) {
            remove_all(path, dev, self);
        }
    }
}

fn run(how: &How, clear: &Clear) {
    let (progress, freed) = (clear.progress, clear.freed);
    match how {
        How::Contents(dirs, skip) => {
            for dir in dirs {
                // Read only real folders inside home, never through a symlink.
                if clear.guard.check(dir).is_none()
                    || fs::symlink_metadata(dir).is_ok_and(|m| !m.is_dir())
                {
                    continue;
                }
                let Ok(entries) = fs::read_dir(dir) else {
                    continue;
                };
                let children: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| !skip.contains(p))
                    .collect();
                children.par_iter().for_each(|p| clear.remove(p));
            }
        }
        How::Paths(paths) => {
            paths.par_iter().for_each(|(p, _)| clear.remove(p));
        }
        How::Worktrees(list) => {
            list.par_iter().for_each(|(path, bytes)| {
                if clear.guard.check(path).is_none() {
                    return;
                }
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
                clear.remove(avd);
                clear.remove(&avd.with_extension("ini"));
            }
        }
        How::Docker(docker, args, bytes) => {
            progress.show(docker);
            let ok = Command::new(docker)
                .args(*args)
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                freed(*bytes);
            }
        }
        How::Demo => {}
    }
}

/// Docker's build cache and unused images, as `docker system df` reports
/// them. Volumes and containers are never touched.
fn docker() -> Vec<Found> {
    // Apps opened from Finder don't get the shell's PATH.
    let Some(docker) = [
        "/usr/bin/docker",
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/Applications/Docker.app/Contents/Resources/bin/docker",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.exists()) else {
        return Vec::new();
    };
    let Some(out) = run_for(
        Command::new(&docker).args(["system", "df", "--format", "{{.Type}}\t{{.Reclaimable}}"]),
        Duration::from_secs(5),
    ) else {
        return Vec::new();
    };
    let reclaimable = |kind: &str| {
        out.lines()
            .find_map(|line| line.strip_prefix(kind)?.strip_prefix('\t'))
            .and_then(|r| r.split_whitespace().next())
            .and_then(docker_bytes)
            .unwrap_or(0)
    };
    let cache = reclaimable("Build Cache");
    let images = reclaimable("Images");
    vec![
        Found {
            name: "Docker build cache".into(),
            detail: REBUILT.into(),
            hue: 205.0,
            icon: Source::App("com.docker.docker", None),
            icon_file: None,
            category: Category::System,
            on: true,
            bytes: cache,
            how: How::Docker(docker.clone(), &["builder", "prune", "-af"], cache),
        },
        Found {
            name: "Docker images".into(),
            detail: "Images no container uses. Pulled again when needed.".into(),
            hue: 210.0,
            icon: Source::App("com.docker.docker", None),
            icon_file: None,
            category: Category::System,
            on: false,
            bytes: images,
            how: How::Docker(docker, &["image", "prune", "-af"], images),
        },
    ]
}

/// Docker's sizes, like "1.25GB" or "512kB" (powers of 1000).
fn docker_bytes(size: &str) -> Option<u64> {
    let split = size.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = size.split_at(split);
    let scale = match unit {
        "B" => 1e0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((number.parse::<f64>().ok()? * scale) as u64)
}

/// Stdout of a command that succeeded within `limit`. A stopped Docker
/// daemon can hang the CLI, and detection must not wait on it.
fn run_for(command: &mut Command, limit: Duration) -> Option<String> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            let mut out = String::new();
            std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
            return Some(out);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Deletes `path` and everything in it. Never follows symlinks, and skips
/// anything on another disk (`dev`) or owned by someone else.
fn remove_all(path: &Path, dev: u64, clear: &Clear) {
    let Ok(md) = fs::symlink_metadata(path) else {
        return;
    };
    if md.dev() != dev || md.uid() != clear.guard.uid {
        return;
    }
    if !md.is_dir() {
        clear.progress.show(path);
        if fs::remove_file(path).is_ok() {
            // Same share-per-link counting as the scan.
            (clear.freed)(md.blocks() * 512 / md.nlink().max(1));
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
        children.par_iter().for_each(|p| remove_all(p, dev, clear));
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
    fn clearing_stays_inside_home() {
        let root = std::env::temp_dir().join(format!("bd-guard-{}", std::process::id()));
        let (home, outside) = (root.join("home"), root.join("outside"));
        let cache = home.join(".cache");
        fs::create_dir_all(cache.join("tool")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(cache.join("tool/blob"), "x").unwrap();
        fs::write(outside.join("keep"), "x").unwrap();
        // A link inside a cache, a cache folder that is a link, and a path
        // outside home: none may reach `outside`.
        std::os::unix::fs::symlink(&outside, cache.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("linked")).unwrap();

        let guard = Guard::new(&home).unwrap();
        let progress = Progress::default();
        let clear = Clear {
            guard: &guard,
            progress: &progress,
            freed: &|_| {},
        };
        run(
            &How::Contents(vec![cache.clone(), home.join("linked")], Vec::new()),
            &clear,
        );
        run(&How::Paths(vec![(outside.clone(), 0)]), &clear);
        run(&How::Paths(vec![(home.join("linked/keep"), 0)]), &clear);
        run(&How::Paths(vec![(cache.join("../../outside"), 0)]), &clear);

        assert!(!cache.join("tool").exists());
        assert!(fs::symlink_metadata(cache.join("link")).is_err());
        assert!(outside.join("keep").exists());
        assert!(Guard::new(Path::new("/")).is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_docker_sizes() {
        assert_eq!(docker_bytes("1.25GB"), Some(1_250_000_000));
        assert_eq!(docker_bytes("512kB"), Some(512_000));
        assert_eq!(docker_bytes("0B"), Some(0));
        assert_eq!(docker_bytes("12%"), None);
    }

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
        assert!(
            start.elapsed() < Duration::from_millis(1500),
            "{:?}",
            start.elapsed()
        );
        assert!(most > 1);
        assert_eq!(progress.cleaning(), 0);
        assert!(items.iter().all(|i| i.left() == 0));
    }
}
