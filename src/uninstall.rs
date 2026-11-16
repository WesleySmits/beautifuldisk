//! Removing an app together with the files it keeps. On macOS everything goes
//! to the Trash, so it can be put back. On Linux, Flatpak and Snap remove
//! their own apps and data.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::Image;

use crate::clutter::{dir_size, run_for};

#[derive(Clone)]
pub struct App {
    pub name: String,
    pub icon: Option<Arc<Image>>,
    /// Bundle ID, Flatpak ID or snap name.
    id: String,
    kind: Kind,
    /// The app, then the files it keeps, with their sizes.
    pub files: Vec<(PathBuf, u64)>,
}

#[derive(Clone, Copy)]
enum Kind {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Mac,
    Flatpak {
        user: bool,
    },
    Snap,
    Demo,
}

impl App {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, b)| b).sum()
    }

    /// Whether the app is running, and so can't be removed yet.
    pub fn is_open(&self) -> bool {
        match self.kind {
            Kind::Mac => mac::is_open(&self.id),
            Kind::Flatpak { .. } => command("flatpak", &["ps", "--columns=application"])
                .is_some_and(|out| out.lines().any(|l| l.trim() == self.id)),
            Kind::Snap | Kind::Demo => false,
        }
    }

    /// Removes the app and its files. Blocks until done, which on Linux
    /// includes the password prompt.
    pub fn remove(&self) -> bool {
        match self.kind {
            Kind::Mac => self.files.iter().all(|(path, _)| mac::trash(path)),
            Kind::Flatpak { user } => {
                let scope = if user { "--user" } else { "--system" };
                std::process::Command::new("flatpak")
                    .args(["uninstall", "-y", "--delete-data", scope, &self.id])
                    .status()
                    .is_ok_and(|s| s.success())
            }
            Kind::Snap => std::process::Command::new("pkexec")
                .args(["snap", "remove", "--purge", &self.id])
                .status()
                .is_ok_and(|s| s.success()),
            Kind::Demo => {
                std::thread::sleep(std::time::Duration::from_millis(600));
                true
            }
        }
    }
}

/// Apps you can remove, largest first.
pub fn list(home: &Path, demo: bool) -> Vec<App> {
    let mut apps = if demo {
        demo_apps()
    } else if cfg!(target_os = "macos") {
        mac::list(home)
    } else {
        linux::list(home)
    };
    apps.sort_by_key(|app| std::cmp::Reverse(app.bytes()));
    apps
}

fn command(program: &str, args: &[&str]) -> Option<String> {
    run_for(
        std::process::Command::new(program).args(args),
        std::time::Duration::from_secs(10),
    )
}

/// The paths among `paths` that exist, with their sizes.
fn sized(paths: impl IntoIterator<Item = PathBuf>) -> Vec<(PathBuf, u64)> {
    paths
        .into_iter()
        .filter(|p| fs::symlink_metadata(p).is_ok())
        .map(|p| {
            let size = dir_size(&p);
            (p, size)
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::icons::{self, Source};
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::{NSBundle, NSFileManager, NSString, NSURL};
    use rayon::prelude::*;
    use std::os::unix::fs::MetadataExt;

    pub(super) fn list(home: &Path) -> Vec<App> {
        let found: Vec<(PathBuf, String)> =
            [PathBuf::from("/Applications"), home.join("Applications")]
                .iter()
                .flat_map(|dir| fs::read_dir(dir).into_iter().flatten().flatten())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "app") && removable(p))
                .filter_map(|p| {
                    let bundle = NSBundle::bundleWithPath(&NSString::from_str(p.to_str()?))?;
                    let id = bundle.bundleIdentifier()?.to_string();
                    Some((p, id))
                })
                .filter(|(_, id)| !id.starts_with("com.apple.") && !id.contains("beautifuldisk"))
                .collect();
        let library = home.join("Library");
        let full_disk = crate::access::full_disk(home);
        found
            .into_par_iter()
            .map(|(path, id)| {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                let files = sized(
                    std::iter::once(path.clone()).chain(kept(&library, &id, &name, full_disk)),
                );
                App {
                    icon: icons::load(Source::File, Some(&path)),
                    name,
                    id,
                    kind: Kind::Mac,
                    files,
                }
            })
            .collect()
    }

    /// Apps you own, so moving one to the Trash needs no password. App Store
    /// and installer apps belong to the system.
    fn removable(app: &Path) -> bool {
        let uid = unsafe { libc::getuid() };
        fs::symlink_metadata(app).is_ok_and(|md| md.is_dir() && md.uid() == uid)
    }

    /// Where an app keeps its files, by bundle ID or by its own name.
    fn kept(library: &Path, id: &str, name: &str, full_disk: bool) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        // Electron apps name their folders after the app, often in lowercase.
        for dir in ["Application Support", "Caches", "Logs"] {
            let entries = fs::read_dir(library.join(dir))
                .into_iter()
                .flatten()
                .flatten();
            paths.extend(entries.map(|e| e.path()).filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(id) || n.eq_ignore_ascii_case(name))
            }));
        }
        // Like Application Support/Google/Chrome for Google Chrome.
        if let Some((vendor, app)) = name.split_once(' ') {
            for dir in ["Application Support", "Caches"] {
                paths.push(library.join(dir).join(vendor).join(app));
            }
        }
        paths.extend([
            library.join("HTTPStorages").join(id),
            library
                .join("HTTPStorages")
                .join(format!("{id}.binarycookies")),
            library.join("WebKit").join(id),
            library.join("Cookies").join(format!("{id}.binarycookies")),
            library.join("Preferences").join(format!("{id}.plist")),
            library
                .join("Saved Application State")
                .join(format!("{id}.savedState")),
        ]);
        // Reading Containers without Full Disk Access asks for permission.
        if full_disk {
            paths.push(library.join("Containers").join(id));
        }
        paths
    }

    pub(super) fn is_open(id: &str) -> bool {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(id))
            .count()
            > 0
    }

    pub(super) fn trash(path: &Path) -> bool {
        let Some(path_str) = path.to_str() else {
            return false;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, None)
            .is_ok()
            // Already gone counts as done.
            || fs::symlink_metadata(path).is_err()
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    use super::*;

    pub(super) fn list(_home: &Path) -> Vec<App> {
        Vec::new()
    }

    pub(super) fn is_open(_id: &str) -> bool {
        false
    }

    pub(super) fn trash(_path: &Path) -> bool {
        false
    }
}

mod linux {
    use super::*;

    pub(super) fn list(home: &Path) -> Vec<App> {
        let mut apps = Vec::new();
        let flatpaks = command(
            "flatpak",
            &["list", "--app", "--columns=application,name,installation"],
        );
        for line in flatpaks.iter().flat_map(|out| out.lines()) {
            let mut cols = line.split('\t');
            let (Some(id), Some(name), Some(place)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let user = place.trim() == "user";
            let installed = if user {
                home.join(".local/share/flatpak/app").join(id)
            } else {
                Path::new("/var/lib/flatpak/app").join(id)
            };
            apps.push(App {
                name: name.trim().to_owned(),
                icon: None,
                id: id.trim().to_owned(),
                kind: Kind::Flatpak { user },
                files: sized([installed, home.join(".var/app").join(id)]),
            });
        }
        // Snaps with a menu entry are apps; the rest are runtimes and themes.
        let desktop = Path::new("/var/lib/snapd/desktop/applications");
        let mut snaps: Vec<(String, String)> = fs::read_dir(desktop)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let file = e.file_name().into_string().ok()?;
                file.strip_suffix(".desktop")?;
                let snap = file.split('_').next()?.to_owned();
                let text = fs::read_to_string(e.path()).unwrap_or_default();
                let name = text
                    .lines()
                    .find_map(|l| l.strip_prefix("Name="))
                    .unwrap_or(&snap)
                    .to_owned();
                Some((snap, name))
            })
            .collect();
        snaps.sort();
        snaps.dedup_by(|a, b| a.0 == b.0);
        for (snap, name) in snaps {
            let images = fs::read_dir("/var/lib/snapd/snaps")
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&format!("{snap}_")) && n.ends_with(".snap"))
                });
            let files = sized(images.chain([home.join("snap").join(&snap)]));
            apps.push(App {
                name,
                icon: None,
                id: snap,
                kind: Kind::Snap,
                files,
            });
        }
        apps
    }
}

fn demo_apps() -> Vec<App> {
    let app = |name: &str, files: &[(&str, u64)]| App {
        name: name.into(),
        icon: None,
        id: name.into(),
        kind: Kind::Demo,
        files: files.iter().map(|&(p, b)| (PathBuf::from(p), b)).collect(),
    };
    vec![
        app(
            "Spotify",
            &[
                ("/Applications/Spotify.app", 410_000_000),
                ("~/Library/Application Support/Spotify", 2_300_000_000),
                ("~/Library/Caches/com.spotify.client", 980_000_000),
                ("~/Library/Preferences/com.spotify.client.plist", 8_000),
            ],
        ),
        app(
            "Slack",
            &[
                ("/Applications/Slack.app", 520_000_000),
                ("~/Library/Application Support/Slack", 1_100_000_000),
                (
                    "~/Library/Saved Application State/com.tinyspeck.slackmacgap.savedState",
                    90_000,
                ),
            ],
        ),
        app(
            "Zoom",
            &[
                ("/Applications/zoom.us.app", 380_000_000),
                ("~/Library/Application Support/zoom.us", 160_000_000),
                ("~/Library/Logs/zoom.us", 42_000_000),
            ],
        ),
    ]
}
