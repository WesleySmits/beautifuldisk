//! Reads the macOS FSEvents journal: which folders changed since an event ID.
//! The journal lives on disk, so it covers time the app wasn't running.

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2_core_foundation::{CFArray, CFString};
use objc2_core_services::*;

pub struct Change {
    pub path: PathBuf,
    /// The journal lost detail below this folder; rescan all of it.
    pub recursive: bool,
}

pub fn current_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

struct State {
    changes: Mutex<Vec<Change>>,
    /// Set when the journal can't be trusted (dropped events, remounts).
    unreliable: Mutex<bool>,
    done: Mutex<Option<mpsc::Sender<()>>>,
}

// The stream holds its own reference to the state and drops it when it is
// freed, so a callback still in flight after `changes_since` returns never
// sees freed memory.
unsafe extern "C-unwind" fn retain(info: *const c_void) -> *const c_void {
    unsafe { Arc::increment_strong_count(info.cast::<State>()) };
    info
}

unsafe extern "C-unwind" fn release(info: *const c_void) {
    unsafe { Arc::decrement_strong_count(info.cast::<State>()) };
}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<FSEventStreamEventFlags>,
    _ids: NonNull<FSEventStreamEventId>,
) {
    let state = unsafe { &*info.cast::<State>() };
    let paths = paths.as_ptr().cast::<*const c_char>();
    let untrusted = kFSEventStreamEventFlagUserDropped
        | kFSEventStreamEventFlagKernelDropped
        | kFSEventStreamEventFlagEventIdsWrapped
        | kFSEventStreamEventFlagRootChanged
        | kFSEventStreamEventFlagMount
        | kFSEventStreamEventFlagUnmount;
    for i in 0..count {
        let flag = unsafe { *flags.as_ptr().add(i) };
        if flag & kFSEventStreamEventFlagHistoryDone != 0 {
            if let Some(done) = state.done.lock().unwrap().take() {
                let _ = done.send(());
            }
            continue;
        }
        if flag & untrusted != 0 {
            *state.unreliable.lock().unwrap() = true;
            continue;
        }
        let path = unsafe { CStr::from_ptr(*paths.add(i)) }.to_string_lossy();
        state.changes.lock().unwrap().push(Change {
            path: PathBuf::from(path.trim_end_matches('/')),
            recursive: flag & kFSEventStreamEventFlagMustScanSubDirs != 0,
        });
    }
}

/// Folders under `roots` that changed since `since`, or `None` when the
/// journal can't answer reliably and everything must be rescanned.
pub fn changes_since(since: u64, roots: &[PathBuf]) -> Option<Vec<Change>> {
    let (done_tx, done_rx) = mpsc::channel();
    let state = Arc::new(State {
        changes: Mutex::default(),
        unreliable: Mutex::new(false),
        done: Mutex::new(Some(done_tx)),
    });
    let paths: Vec<_> = roots
        .iter()
        .map(|p| CFString::from_str(&p.to_string_lossy()))
        .collect();
    let paths = CFArray::from_retained_objects(&paths);
    let mut context = FSEventStreamContext {
        version: 0,
        info: Arc::as_ptr(&state).cast_mut().cast(),
        retain: Some(retain),
        release: Some(release),
        copyDescription: None,
    };
    let queue = DispatchQueue::new("beautifuldisk.fsevents", None);
    let finished = unsafe {
        let stream = FSEventStreamCreate(
            None,
            Some(callback),
            &mut context,
            paths.as_ref(),
            since,
            0.0,
            kFSEventStreamCreateFlagNoDefer,
        );
        if stream.is_null() {
            return None;
        }
        FSEventStreamSetDispatchQueue(stream, Some(&queue));
        let finished =
            FSEventStreamStart(stream) && done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
        finished
    };
    if !finished || *state.unreliable.lock().unwrap() {
        return None;
    }
    Some(std::mem::take(&mut *state.changes.lock().unwrap()))
}
