//! Updates from GitHub releases.
//!
//! macOS: Sparkle, bundled in the app. It asks once whether to check on its
//! own, offers to install updates automatically, and reads its feed from the
//! newest release. Linux: self_update swaps the binary for the newest
//! release's.

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub use linux::*;

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::OnceCell;

    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSBundle, NSString};

    thread_local! {
        static UPDATER: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
    }

    /// Starts Sparkle. Call on the main thread once the app has launched.
    /// Outside the app bundle (`cargo run`) there is no Sparkle, so nothing
    /// happens.
    pub fn start() {
        UPDATER.with(|updater| {
            updater.get_or_init(load);
        });
    }

    /// Shows Sparkle's "Check for Updates" window.
    pub fn check() {
        UPDATER.with(|updater| {
            if let Some(Some(controller)) = updater.get() {
                let sender: Option<&AnyObject> = None;
                // SAFETY: `checkForUpdates:` is SPUStandardUpdaterController's
                // menu action and takes any sender.
                let _: () = unsafe { msg_send![&**controller, checkForUpdates: sender] };
            }
        });
    }

    fn load() -> Option<Retained<AnyObject>> {
        let app = NSBundle::mainBundle().bundlePath().to_string();
        let path = format!("{app}/Contents/Frameworks/Sparkle.framework");
        let sparkle = NSBundle::bundleWithPath(&NSString::from_str(&path))?;
        // SAFETY: loading runs Sparkle's initializers, which have no
        // requirements of their own.
        if !unsafe { sparkle.load() } {
            return None;
        }
        let class = AnyClass::get(c"SPUStandardUpdaterController")?;
        let delegate: Option<&AnyObject> = None;
        // SAFETY: Sparkle's documented initializer; nil delegates use the
        // standard behaviour and UI.
        unsafe {
            let controller: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![
                controller,
                initWithStartingUpdater: true,
                updaterDelegate: delegate,
                userDriverDelegate: delegate
            ]
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod linux {
    use std::path::PathBuf;

    use self_update::backends::github::Update;

    fn updater() -> Option<Update> {
        Update::configure()
            .repo_owner("vkpdeveloper")
            .repo_name("beautifuldisk")
            .bin_name("beautifuldisk")
            // Matches the release's beautiful-disk-linux-x86_64.tar.gz.
            .target(&format!("linux-{}", std::env::consts::ARCH))
            .current_version(env!("CARGO_PKG_VERSION"))
            .show_download_progress(false)
            .show_output(false)
            .no_confirm(true)
            .build()
            .ok()
    }

    /// The newest release's version, if it is newer than this one. Blocks on
    /// the network.
    pub fn newer() -> Option<String> {
        let release = updater()?.is_update_available().ok()??;
        Some(release.version().to_owned())
    }

    /// Replaces this binary with the newest release's and returns where it
    /// is, to start it again. Blocks on the network.
    pub fn install() -> Option<PathBuf> {
        // Read before the swap: afterwards the running binary is unlinked.
        let path = std::env::current_exe().ok()?;
        updater()?.update().ok()?;
        Some(path)
    }
}
