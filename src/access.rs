//! The folders macOS guards with a permission prompt, and which of them the
//! user chose to scan.
//!
//! Reading Desktop, Documents or Downloads makes macOS ask the user, once per
//! folder. A scan that walks them all at launch fires every prompt at once,
//! so the first launch lists them instead and asks only when the user picks
//! one. Folders the user skipped are left out of the scan, so they never
//! prompt.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Folder {
    Desktop,
    Documents,
    Downloads,
}

impl Folder {
    pub const ALL: [Folder; 3] = [Folder::Desktop, Folder::Documents, Folder::Downloads];

    pub fn name(self) -> &'static str {
        match self {
            Folder::Desktop => "Desktop",
            Folder::Documents => "Documents",
            Folder::Downloads => "Downloads",
        }
    }

    pub fn path(self, home: &Path) -> PathBuf {
        home.join(self.name())
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Folders the scan may read. Everything until the app says otherwise, so
/// Linux and the demo never skip anything.
static ALLOWED: AtomicU8 = AtomicU8::new(0b111);

pub fn allowed(folder: Folder) -> bool {
    ALLOWED.load(Relaxed) & folder.bit() != 0
}

/// Whether the scan may walk `path`: false only for a guarded folder the
/// user skipped.
pub fn scannable(home: &Path, path: &Path) -> bool {
    scannable_with(ALLOWED.load(Relaxed), home, path)
}

fn scannable_with(bits: u8, home: &Path, path: &Path) -> bool {
    Folder::ALL
        .iter()
        .all(|&f| bits & f.bit() != 0 || f.path(home) != path)
}

fn bits(folders: &[Folder]) -> u8 {
    folders.iter().fold(0, |bits, f| bits | f.bit())
}

fn set(folders: &[Folder]) {
    ALLOWED.store(bits(folders), Relaxed);
}

fn saved_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/beautifuldisk/access")
}

/// Folders allowed so far while the first launch list is open, so quitting
/// midway (as macOS asks after Full Disk Access) doesn't lose them.
fn progress_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/beautifuldisk/access-progress")
}

fn read(path: &Path) -> Option<Vec<Folder>> {
    let saved = fs::read_to_string(path).ok()?;
    Some(
        Folder::ALL
            .into_iter()
            .filter(|f| saved.lines().any(|line| line == f.name()))
            .collect(),
    )
}

fn write(path: &Path, folders: &[Folder]) {
    let text: String = folders.iter().map(|f| format!("{}\n", f.name())).collect();
    let written = fs::create_dir_all(path.parent().unwrap()).and_then(|_| fs::write(path, text));
    if let Err(err) = written {
        eprintln!("could not save folder choices: {err}");
    }
}

/// Loads the user's choices. While the first launch list is still to be
/// shown, returns the folders already allowed in it.
pub fn load(home: &Path) -> Option<Vec<Folder>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // Full Disk Access covers every folder without a prompt.
    if full_disk(home) {
        set(&Folder::ALL);
        return None;
    }
    if let Some(folders) = read(&saved_path(home)) {
        set(&folders);
        return None;
    }
    set(&[]);
    Some(read(&progress_path(home)).unwrap_or_default())
}

/// Remembers the folders allowed so far in the first launch list.
pub fn save_progress(home: &Path, folders: &[Folder]) {
    write(&progress_path(home), folders);
}

/// Saves which folders to scan and applies them to the next scan.
pub fn save(home: &Path, folders: &[Folder]) {
    set(folders);
    write(&saved_path(home), folders);
    let _ = fs::remove_file(progress_path(home));
}

/// Reads `folder`, which makes macOS ask the first time. Blocks until the
/// user answers. True if the app may read it.
pub fn request(home: &Path, folder: Folder) -> bool {
    match fs::read_dir(folder.path(home)) {
        Ok(_) => true,
        // Nothing there to scan, and nothing to prompt for.
        Err(err) => err.kind() == ErrorKind::NotFound,
    }
}

/// Whether the app has Full Disk Access. Safari's and Mail's folders are
/// readable only with it, and reading them never prompts. (The user's
/// permissions database stays closed even with it on recent macOS.)
pub fn full_disk(home: &Path) -> bool {
    ["Library/Safari", "Library/Mail"]
        .iter()
        .any(|dir| fs::read_dir(home.join(dir)).is_ok())
        || fs::File::open("/Library/Application Support/com.apple.TCC/TCC.db").is_ok()
}

pub const FULL_DISK_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
pub const FOLDER_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_folders_stay_out_of_the_scan() {
        let home = Path::new("/Users/someone");
        let only_documents = bits(&[Folder::Documents]);
        assert!(scannable_with(
            only_documents,
            home,
            &home.join("Documents")
        ));
        assert!(!scannable_with(only_documents, home, &home.join("Desktop")));
        assert!(!scannable_with(
            only_documents,
            home,
            &home.join("Downloads")
        ));
        assert!(scannable_with(
            only_documents,
            home,
            &home.join("Developer")
        ));
    }
}
