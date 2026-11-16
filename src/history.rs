//! A plain text record of everything cleared, so you can look back at what
//! went. One entry per item, with the paths and commands it touched.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::clutter::Item;
use crate::disk;

pub fn path() -> PathBuf {
    let home = disk::home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/beautifuldisk/history.log")
    } else {
        // Not under ~/.cache, which App caches empties.
        home.join(".local/state/beautifuldisk/history.log")
    }
}

/// Appends what clearing `item` did. `open` names the app that kept it.
pub fn record(item: &Item, freed: u64, open: Option<&str>) {
    let mut entry = match open {
        Some(app) => format!("{}  Skipped {}, {app} was open\n", now(), item.name),
        None => format!(
            "{}  Cleared {}, {:.1} GB\n",
            now(),
            item.name,
            freed as f64 / 1e9
        ),
    };
    for t in item.targets() {
        entry += &format!("    {}  ({})\n", t.what, t.how);
    }
    let path = path();
    // One write per entry, so items clearing at once don't interleave.
    let written = fs::create_dir_all(path.parent().unwrap()).and_then(|_| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(entry.as_bytes())
    });
    if let Err(err) = written {
        eprintln!("could not write clearing history: {err}");
    }
}

/// Local time like "2026-11-17 14:02".
fn now() -> String {
    // SAFETY: localtime_r and strftime write only into the buffers given.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(
            buf.as_mut_ptr().cast(),
            buf.len(),
            c"%Y-%m-%d %H:%M".as_ptr(),
            &tm,
        );
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}
