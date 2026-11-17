//! App data outside the scanned folders: Electron caches, old coding agent
//! versions, Chrome's on-device models, Steam, Flatpak and Snap caches and
//! more. Sized by reading the folders directly, since the scan skips them.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Found, How, Pick, category_of, count, dir_size};
use crate::access;
use crate::disk::Category;
use crate::icons::{Logo, Source};

/// Folder names Chromium and Electron rebuild on their own.
const CHROMIUM_CACHES: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GraphiteDawnCache",
    "GrShaderCache",
    "ShaderCache",
    "component_crx_cache",
    "extensions_crx_cache",
    "Crashpad/completed",
];

/// Files a running Chromium or Electron app holds in its data folder.
const LOCKS: &[&str] = &["SingletonLock", "code.lock"];

/// Whether the app with data in `dir` is running.
pub(super) fn locked(dir: &Path) -> bool {
    LOCKS
        .iter()
        .any(|lock| fs::symlink_metadata(dir.join(lock)).is_ok())
}

pub(super) fn find(home: &Path, roots: &[(PathBuf, Category)]) -> Vec<Found> {
    let mut found = Vec::new();
    let mac = cfg!(target_os = "macos");
    let support = home.join(if mac {
        "Library/Application Support"
    } else {
        ".config"
    });
    let category = |path: &Path| category_of(roots, path);

    found.extend(electron(&support, category(&support)));
    found.extend(agent_versions(home, roots));

    let contents = |paths: Vec<PathBuf>| {
        let bytes = paths.iter().map(|p| dir_size(p)).sum();
        (bytes, How::Contents(paths, Vec::new()))
    };

    let codex = support.join("Codex/Crashpad/pending");
    let (bytes, how) = contents(vec![codex.clone()]);
    found.push(Found {
        name: "Codex crash reports".into(),
        detail: "Crash reports waiting to be sent.".into(),
        hue: 160.0,
        icon: Source::App("com.openai.codex", Some(Logo::OpenAi)),
        icon_file: None,
        category: category(&codex),
        pick: Pick::On,
        bytes,
        how,
    });

    // Chrome downloads these again when a feature needs them.
    let chromes: &[&str] = if mac {
        &["Google/Chrome", "Chromium"]
    } else {
        &["google-chrome", "chromium"]
    };
    let models: Vec<(PathBuf, u64)> = chromes
        .iter()
        .flat_map(|c| children(&support.join(c)))
        .filter(|p| {
            p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with("OptGuideOnDevice") || n == "optimization_guide_model_store"
            })
        })
        .map(|p| {
            let bytes = dir_size(&p);
            (p, bytes)
        })
        .collect();
    found.push(Found {
        name: "Chrome AI models".into(),
        detail: "Models Chrome downloaded for its AI features. Downloaded again when needed."
            .into(),
        hue: 220.0,
        icon: Source::App("com.google.Chrome", None),
        icon_file: None,
        category: category(&support),
        pick: Pick::On,
        bytes: models.iter().map(|(_, b)| b).sum(),
        how: How::Paths(models),
    });

    let steam_roots = if mac {
        vec![support.join("Steam")]
    } else {
        vec![
            home.join(".local/share/Steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        ]
    };
    let steam: Vec<PathBuf> = steam_roots
        .iter()
        .flat_map(|root| {
            [
                "appcache/httpcache",
                "config/htmlcache",
                "steamapps/shadercache",
                "depotcache",
                "logs",
            ]
            .map(|p| root.join(p))
        })
        .collect();
    let (bytes, how) = contents(steam);
    found.push(Found {
        name: "Steam caches".into(),
        detail: "Web pages, shaders and logs. Rebuilt by Steam.".into(),
        hue: 215.0,
        icon: Source::App("com.valvesoftware.steam", None),
        icon_file: None,
        category: category(&steam_roots[0]),
        pick: Pick::On,
        bytes,
        how,
    });

    if mac {
        let updates: Vec<PathBuf> = ["iPhone", "iPad", "iPod", "Apple TV"]
            .iter()
            .map(|d| home.join(format!("Library/iTunes/{d} Software Updates")))
            .collect();
        let (bytes, how) = contents(updates);
        found.push(Found {
            name: "iPhone and iPad updates".into(),
            detail: "Software updates Finder downloaded for your devices.".into(),
            hue: 210.0,
            icon: Source::App("com.apple.finder", Some(Logo::Apple)),
            icon_file: None,
            category: Category::System,
            pick: Pick::On,
            bytes,
            how,
        });
        // Other apps' containers and Messages are readable only with Full
        // Disk Access; without it, reading them would make macOS ask.
        if access::full_disk(home) {
            found.extend(containers(home));
            let previews: Vec<PathBuf> = [
                "Library/Messages/Caches/Previews/Attachments",
                "Library/Messages/Caches/Previews/StickerCache",
                "Library/Messages/StickerCache",
            ]
            .iter()
            .map(|p| home.join(p))
            .collect();
            let (bytes, how) = contents(previews);
            found.push(Found {
                name: "Messages previews".into(),
                detail: "Thumbnails of attachments. Your attachments are kept.".into(),
                hue: 130.0,
                icon: Source::App("com.apple.MobileSMS", None),
                icon_file: None,
                category: Category::System,
                pick: Pick::On,
                bytes,
                how,
            });
        }
    } else {
        found.extend(sandboxed(home, roots));
    }

    found.extend(brew());
    found
}

/// Subfolders of `dir`, or nothing if it can't be read.
pub(super) fn children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).map_or(Vec::new(), |entries| {
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect()
    })
}

/// Chromium caches inside every Electron app's data folder.
fn electron(support: &Path, category: Category) -> Vec<Found> {
    let mut apps = Vec::new();
    let mut names = Vec::new();
    for dir in children(support) {
        // Only folders that look like Chromium's, so a "Cache" folder of some
        // other app is never mistaken for one.
        let chromium = ["GPUCache", "Code Cache"]
            .iter()
            .any(|m| dir.join(m).is_dir());
        if !chromium {
            continue;
        }
        let leaves: Vec<(PathBuf, u64)> = CHROMIUM_CACHES
            .iter()
            .map(|leaf| dir.join(leaf))
            .filter(|p| p.is_dir())
            .map(|p| {
                let bytes = dir_size(&p);
                (p, bytes)
            })
            .filter(|(_, b)| *b > 0)
            .collect();
        if leaves.is_empty() {
            continue;
        }
        if let Some(name) = dir.file_name() {
            names.push(name.to_string_lossy().into_owned());
        }
        apps.push((dir, leaves));
    }
    let bytes = apps
        .iter()
        .flat_map(|(_, leaves)| leaves.iter().map(|(_, b)| b))
        .sum();
    let detail = match names.len() {
        0 => String::new(),
        1..=3 => format!("{}. Apps that are open are skipped.", names.join(", ")),
        n => format!(
            "{} and {} more. Apps that are open are skipped.",
            names[..3].join(", "),
            n - 3
        ),
    };
    vec![Found {
        name: "Electron app caches".into(),
        detail,
        hue: 195.0,
        icon: Source::File,
        icon_file: Some(support.to_path_buf()),
        category,
        pick: Pick::On,
        bytes,
        how: How::Unlocked(apps),
    }]
}

/// Coding agents that keep every version they updated to. The one in use
/// and the one before it stay.
fn agent_versions(home: &Path, roots: &[(PathBuf, Category)]) -> Vec<Found> {
    const AGENTS: &[(&str, &str, &str, Source)] = &[
        (
            "Claude Code",
            ".local/share/claude/versions",
            ".local/bin/claude",
            Source::App("com.anthropic.claudefordesktop", Some(Logo::Claude)),
        ),
        (
            "Cursor Agent",
            ".local/share/cursor-agent/versions",
            ".local/bin/cursor-agent",
            Source::Logo(Logo::Git),
        ),
        (
            "Copilot CLI",
            ".copilot/pkg/universal",
            ".local/bin/copilot",
            Source::Logo(Logo::Git),
        ),
    ];
    let mut found = Vec::new();
    for (tool, versions, link, icon) in AGENTS {
        let dir = home.join(versions);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        // Without a working link to the one in use, keep them all.
        let Ok(active) = fs::canonicalize(home.join(link)) else {
            continue;
        };
        let mut all: Vec<(PathBuf, std::time::SystemTime)> = entries
            .flatten()
            .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
            .collect();
        let in_use = |p: &Path| fs::canonicalize(p).is_ok_and(|p| active.starts_with(p));
        if !all.iter().any(|(p, _)| in_use(p)) {
            continue;
        }
        all.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
        let mut kept_previous = false;
        let old: Vec<(PathBuf, u64)> = all
            .into_iter()
            .filter(|(p, _)| {
                if in_use(p) {
                    return false;
                }
                if !kept_previous {
                    kept_previous = true;
                    return false;
                }
                true
            })
            .map(|(p, _)| {
                let bytes = dir_size(&p);
                (p, bytes)
            })
            .collect();
        found.push(Found {
            name: format!("Old {tool} versions"),
            detail: format!(
                "{}. Keeps the one you use and the one before it.",
                count(old.len(), "version", "versions")
            ),
            hue: 25.0,
            icon: *icon,
            icon_file: None,
            category: category_of(roots, &dir),
            pick: Pick::On,
            bytes: old.iter().map(|(_, b)| b).sum(),
            how: How::Paths(old),
        });
    }
    found
}

/// Caches apps keep inside their sandbox containers, outside Library/Caches.
fn containers(home: &Path) -> Vec<Found> {
    let caches: Vec<PathBuf> = children(&home.join("Library/Containers"))
        .into_iter()
        .map(|c| c.join("Data/Library/Caches"))
        .filter(|c| c.is_dir())
        .collect();
    let skip: Vec<PathBuf> = caches
        .iter()
        .flat_map(|c| children(c))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| super::protected_cache(&n.to_string_lossy()))
        })
        .collect();
    let bytes = caches
        .iter()
        .map(|c| dir_size(c))
        .sum::<u64>()
        .saturating_sub(skip.iter().map(|s| dir_size(s)).sum());
    vec![Found {
        name: "Sandboxed app caches".into(),
        detail: "Caches App Store apps keep in their own folders.".into(),
        hue: 35.0,
        icon: Source::File,
        icon_file: Some(home.join("Library/Containers")),
        category: Category::System,
        pick: Pick::On,
        bytes,
        how: How::Contents(caches, skip),
    }]
}

/// Flatpak and Snap apps keep their caches in their own folders.
fn sandboxed(home: &Path, roots: &[(PathBuf, Category)]) -> Vec<Found> {
    let caches: Vec<PathBuf> = children(&home.join(".var/app"))
        .into_iter()
        .map(|app| app.join("cache"))
        .chain(
            children(&home.join("snap"))
                .into_iter()
                .map(|app| app.join("common/.cache")),
        )
        .filter(|c| c.is_dir())
        .collect();
    let bytes = caches.iter().map(|c| dir_size(c)).sum();
    vec![Found {
        name: "Flatpak and Snap caches".into(),
        detail: "Caches of apps from Flatpak and Snap.".into(),
        hue: 35.0,
        icon: Source::File,
        icon_file: None,
        category: category_of(roots, &home.join(".var")),
        pick: Pick::On,
        bytes,
        how: How::Contents(caches, Vec::new()),
    }]
}

/// Old versions of what Homebrew installed, removed by Homebrew itself.
/// Sized by reading the Cellar: `brew cleanup --dry-run` gives the same
/// answer but takes seconds.
fn brew() -> Vec<Found> {
    let Some(brew) = [
        "/opt/homebrew/bin/brew",
        "/usr/local/bin/brew",
        "/home/linuxbrew/.linuxbrew/bin/brew",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.exists()) else {
        return Vec::new();
    };
    let Some(prefix) = brew.parent().and_then(Path::parent) else {
        return Vec::new();
    };
    let bytes = old_kegs(prefix).iter().map(|keg| dir_size(keg)).sum();
    vec![Found {
        name: "Old Homebrew versions".into(),
        detail: "Versions Homebrew replaced with newer ones.".into(),
        hue: 50.0,
        icon: Source::Logo(Logo::HomeBrew),
        icon_file: None,
        category: Category::System,
        pick: Pick::Dev,
        bytes,
        how: How::Run(brew, &["cleanup", "--prune=all"], bytes),
    }]
}

/// Installed versions `brew cleanup` removes: all but the one in use, except
/// linked or pinned ones, ones another keg needs (`.keepme`), and ones
/// without an install receipt, which Homebrew leaves alone.
fn old_kegs(prefix: &Path) -> Vec<PathBuf> {
    let current = |dir: &str, name: &OsStr| fs::canonicalize(prefix.join(dir).join(name)).ok();
    let mut old = Vec::new();
    for formula in children(&prefix.join("Cellar")) {
        let Some(name) = formula.file_name() else {
            continue;
        };
        // Without a version in use, Homebrew can't tell which to keep.
        let Some(in_use) = current("opt", name) else {
            continue;
        };
        let keep = [
            Some(in_use),
            current("var/homebrew/linked", name),
            current("var/homebrew/pinned", name),
        ];
        old.extend(children(&formula).into_iter().filter(|keg| {
            let keg_path = fs::canonicalize(keg).ok();
            !keep.contains(&keg_path)
                && keg.join("INSTALL_RECEIPT.json").is_file()
                && !keg.join(".keepme").exists()
        }));
    }
    old
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_old_homebrew_versions() {
        let prefix = std::env::temp_dir().join(format!("bd-brew-{}", std::process::id()));
        let keg = |name: &str, version: &str, receipt: bool| {
            let dir = prefix.join("Cellar").join(name).join(version);
            fs::create_dir_all(&dir).unwrap();
            if receipt {
                fs::write(dir.join("INSTALL_RECEIPT.json"), "{}").unwrap();
            }
        };
        let link = |dir: &str, name: &str, version: &str| {
            let at = prefix.join(dir);
            fs::create_dir_all(&at).unwrap();
            std::os::unix::fs::symlink(
                prefix.join("Cellar").join(name).join(version),
                at.join(name),
            )
            .unwrap();
        };
        keg("go", "1.21", true);
        keg("go", "1.22", true);
        link("opt", "go", "1.22");
        // No receipt: Homebrew leaves it.
        keg("tailscale", "1.0", false);
        keg("tailscale", "1.1", true);
        link("opt", "tailscale", "1.1");
        keg("node", "20", true);
        keg("node", "22", true);
        link("opt", "node", "22");
        link("var/homebrew/pinned", "node", "20");

        let old = old_kegs(&prefix);
        fs::remove_dir_all(&prefix).unwrap();
        assert_eq!(old, [prefix.join("Cellar/go/1.21")]);
    }
}
