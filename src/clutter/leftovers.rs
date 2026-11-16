//! Data that apps leave behind after they are deleted. On macOS, folders
//! named after an app's bundle ID when no app with that ID is installed. On
//! Linux, Flatpak and Snap data for apps that were removed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{Found, How, Pick, category_of, count, dir_size, run_for};
use crate::disk::Category;
use crate::icons::Source;

/// Recently used folders may belong to something still around, like a helper
/// tool, so only ones untouched this long count.
const UNTOUCHED: Duration = Duration::from_secs(30 * 86_400);

pub(super) fn find(home: &Path, roots: &[(PathBuf, Category)]) -> Vec<Found> {
    let gone = if cfg!(target_os = "macos") {
        mac(home)
    } else {
        linux(home)
    };
    let gone: Vec<(PathBuf, u64)> = gone
        .into_iter()
        .filter(|p| untouched(p))
        .map(|p| {
            let size = dir_size(&p);
            (p, size)
        })
        .collect();
    let apps = {
        // Named by bundle ID, Flatpak ID or snap name.
        let mut names: Vec<_> = gone.iter().filter_map(|(p, _)| p.file_name()).collect();
        names.sort();
        names.dedup();
        names.len()
    };
    let Some((first, _)) = gone.first() else {
        return Vec::new();
    };
    vec![Found {
        name: "Deleted apps' data".into(),
        detail: format!(
            "Left behind by {} that are no longer installed.",
            count(apps, "app", "apps")
        ),
        hue: 330.0,
        icon: Source::File,
        icon_file: None,
        category: category_of(roots, first),
        pick: Pick::Off,
        bytes: gone.iter().map(|(_, b)| b).sum(),
        how: How::Paths(gone),
    }]
}

fn untouched(path: &Path) -> bool {
    let newest = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .chain(fs::symlink_metadata(path).ok())
        .filter_map(|md| md.modified().ok())
        .max();
    newest.is_some_and(|t| {
        SystemTime::now()
            .duration_since(t)
            .is_ok_and(|age| age > UNTOUCHED)
    })
}

#[cfg(target_os = "macos")]
/// Whether `name` looks like a bundle ID, such as com.vendor.App.
fn bundle_like(name: &str) -> bool {
    const PREFIXES: &[&str] = &["com.", "org.", "net.", "io.", "app.", "dev.", "co."];
    PREFIXES.iter().any(|p| name.starts_with(p))
        && name.split('.').count() >= 3
        && !name.starts_with("com.apple.")
        && !super::protected_cache(name)
}

#[cfg(target_os = "macos")]
fn mac(home: &Path) -> Vec<PathBuf> {
    let library = home.join("Library");
    let mut gone = Vec::new();
    // Not Preferences: libraries inside apps name their files like bundle IDs.
    for dir in ["Application Support", "HTTPStorages", "WebKit"] {
        let Ok(entries) = fs::read_dir(library.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if bundle_like(name) && !installed(name) {
                gone.push(path);
            }
        }
    }
    gone
}

/// Whether an app with bundle ID `id`, or one it is a helper of (like
/// com.vendor.App.ShipIt), is installed.
#[cfg(target_os = "macos")]
fn installed(id: &str) -> bool {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let workspace = NSWorkspace::sharedWorkspace();
    let parts: Vec<&str> = id.split('.').collect();
    (3..=parts.len()).any(|n| {
        let id = parts[..n].join(".");
        // Launch Services can still remember an app that was deleted.
        workspace
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(&id))
            .and_then(|url| url.path())
            .is_some_and(|path| Path::new(&path.to_string()).exists())
    })
}

#[cfg(not(target_os = "macos"))]
fn mac(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

fn linux(home: &Path) -> Vec<PathBuf> {
    let mut gone = Vec::new();
    // Only when the tool answers: no list means nothing is known to be gone.
    let flatpaks = listed("flatpak", &["list", "--app", "--columns=application"]);
    if let Some(apps) = flatpaks {
        gone.extend(
            super::apps::children(&home.join(".var/app"))
                .into_iter()
                .filter(|p| !is_listed(p, &apps)),
        );
    }
    if let Some(snaps) = listed("snap", &["list"]) {
        let snaps: Vec<String> = snaps
            .iter()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect();
        gone.extend(
            super::apps::children(&home.join("snap"))
                .into_iter()
                .filter(|p| !is_listed(p, &snaps)),
        );
    }
    gone
}

fn is_listed(path: &Path, names: &[String]) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_none_or(|n| names.iter().any(|app| app == n))
}

fn listed(program: &str, args: &[&str]) -> Option<Vec<String>> {
    let out = run_for(
        std::process::Command::new(program).args(args),
        Duration::from_secs(10),
    )?;
    Some(out.lines().map(|l| l.trim().to_owned()).collect())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn only_bundle_ids_count() {
        assert!(bundle_like("com.tinyspeck.slackmacgap"));
        assert!(bundle_like("org.videolan.vlc"));
        assert!(!bundle_like("com.apple.Safari"));
        assert!(!bundle_like("Google"));
        assert!(!bundle_like("com.google"));
    }
}
