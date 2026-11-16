//! Directory listing. On macOS `getattrlistbulk(2)` returns names, types and
//! sizes for a whole batch of entries in one syscall, instead of one
//! `readdir` plus one `lstat` per entry. Linux has no such call, so it reads
//! names with `readdir` and sizes with `fstatat` relative to the open folder.

#[cfg(target_os = "macos")]
use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::ffi::c_void;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

pub struct Entry<'a> {
    pub name: &'a OsStr,
    pub kind: Kind,
    /// Same value as `MetadataExt::dev`.
    pub dev: u64,
    pub links: u32,
    /// Allocated bytes on disk (0 for directories).
    pub size: u64,
}

// `vtype` values from <sys/vnode.h>.
#[cfg(target_os = "macos")]
const VREG: u32 = 1;
#[cfg(target_os = "macos")]
const VDIR: u32 = 2;
#[cfg(target_os = "macos")]
const VLNK: u32 = 5;

#[cfg(target_os = "macos")]
const BUF_SIZE: usize = 256 * 1024;

#[cfg(target_os = "macos")]
thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0; BUF_SIZE]);
}

/// Directories held open at once by [`Dir::open_child`]. Each costs a file
/// descriptor, so past the budget children are opened by path later.
static OPEN: AtomicUsize = AtomicUsize::new(0);
static BUDGET: AtomicUsize = AtomicUsize::new(0);

/// Raises the open-file limit (256 by default for apps run from a terminal)
/// and sizes the budget of held-open directories to half of it.
pub fn raise_fd_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            limit.rlim_cur = limit.rlim_max.min(8192);
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
        }
    }
    BUDGET.store(limit.rlim_cur as usize / 2, Relaxed);
}

const OPEN_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// An open directory.
pub struct Dir {
    fd: libc::c_int,
    counted: bool,
}

impl Drop for Dir {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
        if self.counted {
            OPEN.fetch_sub(1, Relaxed);
        }
    }
}

#[cfg(target_os = "macos")]
fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap())
}

#[cfg(target_os = "macos")]
fn u64_at(buf: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(buf[at..at + 8].try_into().unwrap())
}

impl Dir {
    /// Opens by full path. Does not follow a symlinked `path`.
    pub fn open(path: &Path) -> io::Result<Dir> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let fd = unsafe { libc::open(path.as_ptr(), OPEN_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Dir { fd, counted: false })
    }

    /// Opens a child relative to this directory, resolving one name instead
    /// of every ancestor. Returns `None` when over budget or on error; the
    /// caller opens by path then.
    pub fn open_child(&self, name: &OsStr) -> Option<Dir> {
        if OPEN.fetch_add(1, Relaxed) >= BUDGET.load(Relaxed) {
            OPEN.fetch_sub(1, Relaxed);
            return None;
        }
        let fd = CString::new(name.as_bytes())
            .ok()
            .map(|name| unsafe { libc::openat(self.fd, name.as_ptr(), OPEN_FLAGS) })
            .filter(|fd| *fd >= 0);
        if fd.is_none() {
            OPEN.fetch_sub(1, Relaxed);
        }
        fd.map(|fd| Dir { fd, counted: true })
    }

    /// Calls `f` for every entry.
    #[cfg(target_os = "macos")]
    pub fn read(&self, mut f: impl FnMut(Entry)) -> io::Result<()> {
        let mut attrs = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: libc::ATTR_CMN_RETURNED_ATTRS
                | libc::ATTR_CMN_NAME
                | libc::ATTR_CMN_DEVID
                | libc::ATTR_CMN_OBJTYPE,
            volattr: 0,
            dirattr: 0,
            fileattr: libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE,
            forkattr: 0,
        };

        BUF.with_borrow_mut(|buf| {
            loop {
                // FSOPT_PACK_INVAL_ATTRS gives every record the same layout:
                // attributes an entry lacks (file sizes on a directory) are
                // zero-filled, not omitted.
                let count = unsafe {
                    libc::getattrlistbulk(
                        self.fd,
                        (&raw mut attrs).cast::<c_void>(),
                        buf.as_mut_ptr().cast::<c_void>(),
                        buf.len(),
                        libc::FSOPT_PACK_INVAL_ATTRS as u64,
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                if count == 0 {
                    return Ok(());
                }

                let mut record = 0;
                for _ in 0..count {
                    let rec = &buf[record..];
                    let len = u32_at(rec, 0) as usize;
                    // Layout, in the kernel's fixed order: u32 length,
                    // attribute_set_t (20 bytes), name attrreference_t, dev_t,
                    // fsobj_type_t, u32 link count, off_t alloc size.
                    let name_ref = 4 + 20;
                    let name_offset = u32_at(rec, name_ref) as i32;
                    let name_len = u32_at(rec, name_ref + 4) as usize;
                    let name_start = (name_ref as i64 + name_offset as i64) as usize;
                    // The length includes the trailing NUL.
                    let name = &rec[name_start..name_start + name_len.saturating_sub(1)];
                    let dev = u32_at(rec, 32) as i32;
                    let kind = match u32_at(rec, 36) {
                        VREG => Kind::File,
                        VDIR => Kind::Dir,
                        VLNK => Kind::Symlink,
                        _ => Kind::Other,
                    };
                    f(Entry {
                        name: OsStr::from_bytes(name),
                        kind,
                        // dev_t is signed; sign-extend like std's st_dev conversion.
                        dev: dev as u64,
                        links: u32_at(rec, 40),
                        size: u64_at(rec, 44),
                    });
                    record += len;
                }
            }
        })
    }

    /// Calls `f` for every entry.
    #[cfg(not(target_os = "macos"))]
    pub fn read(&self, mut f: impl FnMut(Entry)) -> io::Result<()> {
        // fdopendir takes the descriptor over, so give it a copy.
        let fd = unsafe { libc::dup(self.fd) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }
        loop {
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            let found = unsafe {
                libc::fstatat(self.fd, name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW)
            };
            if found != 0 {
                continue;
            }
            let kind = match stat.st_mode & libc::S_IFMT {
                libc::S_IFREG => Kind::File,
                libc::S_IFDIR => Kind::Dir,
                libc::S_IFLNK => Kind::Symlink,
                _ => Kind::Other,
            };
            f(Entry {
                name: OsStr::from_bytes(bytes),
                kind,
                dev: stat.st_dev as u64,
                links: stat.st_nlink as u32,
                // Like macOS, folders themselves count as 0 bytes.
                size: if kind == Kind::Dir {
                    0
                } else {
                    stat.st_blocks as u64 * 512
                },
            });
        }
        unsafe { libc::closedir(dir) };
        Ok(())
    }
}
