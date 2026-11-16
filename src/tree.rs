//! The scanned folder tree. A full scan walks every folder; later scans load
//! the saved tree and re-read only the folders FSEvents says changed.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::thread;

use crate::bulk::{Dir, Kind};
use crate::disk::{Category, Progress, Report, roots};
use crate::fsevents;

const NONE: u32 = u32::MAX;
/// Passed as a child's group to mean "this folder starts its own group".
const OWN_GROUP: u32 = u32::MAX - 1;

const SIMULATORS: u8 = 1;
const BUILD_OUTPUT: u8 = 2;

/// Smaller files are not worth naming as a category's largest.
const LARGEST_MIN: u64 = 1_000_000;

fn tags_for(path: &Path, name: &OsStr) -> u8 {
    match name.as_bytes() {
        b"CoreSimulator" => SIMULATORS,
        b"DerivedData" => BUILD_OUTPUT,
        // Cargo marks its target dirs; plain "target" folders may be anything.
        b"target" if path.join("CACHEDIR.TAG").exists() => BUILD_OUTPUT,
        _ => 0,
    }
}

struct Node {
    parent: u32,
    /// The top-level item (an app, a project folder) this folder belongs to.
    group: u32,
    category: Category,
    tags: u8,
    alive: bool,
    /// Bytes of the files directly inside, not counting subfolders.
    own: u64,
    /// Full path for roots, a single name otherwise.
    name: Box<[u8]>,
    largest: Option<(Box<str>, u64)>,
}

pub struct Tree {
    nodes: Vec<Node>,
    /// FSEvents ID from just before the tree was last brought up to date.
    event_id: u64,
}

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        // Directory reads block in the kernel; a few extra threads keep cores busy.
        let threads = thread::available_parallelism().map_or(8, |n| n.get() + n.get() / 2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    })
}

/// What one folder holds, from a single listing.
struct Listing {
    own: u64,
    largest: Option<(Box<str>, u64)>,
    subdirs: Vec<(OsString, Option<Dir>)>,
}

fn list(handle: &Dir, dev: u64) -> Listing {
    let mut listing = Listing {
        own: 0,
        largest: None,
        subdirs: Vec::new(),
    };
    let _ = handle.read(|entry| {
        if entry.dev != dev {
            return;
        }
        if entry.kind == Kind::Dir {
            listing
                .subdirs
                .push((entry.name.to_owned(), handle.open_child(entry.name)));
            return;
        }
        // A hard-linked file counts a share per link, so the links add up to
        // its size once without tracking every inode across the scan.
        let size = entry.size / entry.links.max(1) as u64;
        listing.own += size;
        let bigger = listing
            .largest
            .as_ref()
            .is_none_or(|(_, best)| size > *best);
        if entry.kind == Kind::File && size >= LARGEST_MIN && bigger {
            listing.largest = Some((entry.name.to_string_lossy().into(), size));
        }
    });
    listing
}

struct Walk<'a> {
    nodes: Mutex<Vec<Node>>,
    progress: &'a Progress,
    dirs: AtomicU64,
}

struct Place {
    parent: u32,
    group: u32,
    category: Category,
    tags: u8,
    dev: u64,
}

impl<'a> Walk<'a> {
    /// Adds `path` and everything below it to the tree.
    fn dir<'s>(
        &'s self,
        scope: &rayon::Scope<'s>,
        path: PathBuf,
        name: Box<[u8]>,
        handle: Option<Dir>,
        at: Place,
    ) where
        'a: 's,
    {
        let Some(handle) = handle.or_else(|| Dir::open(&path).ok()) else {
            return;
        };
        if self.dirs.fetch_add(1, Relaxed).is_multiple_of(16) {
            self.progress.show(&path);
        }
        let listing = list(&handle, at.dev);
        drop(handle);
        self.progress.add(at.category, listing.own);
        let index = self.push(Node {
            parent: at.parent,
            group: at.group,
            category: at.category,
            tags: at.tags,
            alive: true,
            own: listing.own,
            name,
            largest: listing.largest,
        });
        self.children(scope, &path, index, listing.subdirs, &at);
    }

    fn push(&self, mut node: Node) -> u32 {
        let mut nodes = self.nodes.lock().unwrap();
        let index = nodes.len() as u32;
        if node.group == OWN_GROUP {
            node.group = index;
        }
        nodes.push(node);
        index
    }

    /// Walks `subdirs` of the folder at `index`, placed at `at`.
    fn children<'s>(
        &'s self,
        scope: &rayon::Scope<'s>,
        path: &Path,
        index: u32,
        subdirs: Vec<(OsString, Option<Dir>)>,
        at: &Place,
    ) where
        'a: 's,
    {
        let group = if at.parent == NONE {
            OWN_GROUP
        } else {
            self.nodes.lock().unwrap()[index as usize].group
        };
        for (name, handle) in subdirs {
            let child = path.join(&name);
            let place = Place {
                parent: index,
                group,
                category: at.category,
                tags: at.tags | tags_for(&child, &name),
                dev: at.dev,
            };
            let name = name.into_vec().into_boxed_slice();
            scope.spawn(move |scope| self.dir(scope, child, name, handle, place));
        }
    }
}

impl Tree {
    /// Walks every category root.
    pub fn scan(home: &Path, progress: &Progress) -> Tree {
        let event_id = fsevents::current_id();
        let state = Walk {
            nodes: Mutex::default(),
            progress,
            dirs: AtomicU64::new(0),
        };
        let walk = &state;
        pool().scope(|scope| {
            for (root, category) in roots(home) {
                let Ok(md) = fs::metadata(&root) else {
                    continue;
                };
                let name = root.as_os_str().as_bytes().into();
                let place = Place {
                    parent: NONE,
                    group: NONE,
                    category,
                    tags: 0,
                    dev: md.dev(),
                };
                scope.spawn(move |scope| walk.dir(scope, root, name, None, place));
            }
        });
        Tree {
            nodes: state.nodes.into_inner().unwrap(),
            event_id,
        }
    }

    /// Re-reads the folders that changed since the tree was saved. Returns
    /// false when FSEvents can't say, and a full scan is needed.
    pub fn refresh(&mut self, home: &Path, progress: &Progress) -> bool {
        let event_id = fsevents::current_id();
        let root_paths: Vec<PathBuf> = roots(home).into_iter().map(|(p, _)| p).collect();
        let Some(changes) = fsevents::changes_since(self.event_id, &root_paths) else {
            return false;
        };
        // A root that appeared or vanished changes the tree's shape at the top.
        let known_roots: HashSet<&[u8]> = self
            .nodes
            .iter()
            .filter(|n| n.parent == NONE)
            .map(|n| &*n.name)
            .collect();
        let present_roots = root_paths.iter().filter(|p| p.is_dir()).count();
        if present_roots != known_roots.len() {
            return false;
        }
        if !changes.is_empty() {
            self.apply(changes, progress);
        }
        self.event_id = event_id;
        true
    }

    fn apply(&mut self, changes: Vec<fsevents::Change>, progress: &Progress) {
        let children = Children::new(&self.nodes);
        let roots: Vec<u32> = (0..self.nodes.len() as u32)
            .filter(|&i| self.nodes[i as usize].parent == NONE && self.nodes[i as usize].alive)
            .collect();

        // Map each change to the nearest folder we know. A new folder shows
        // up as a change to its parent, whose rescan walks it.
        let mut todo: HashMap<u32, bool> = HashMap::new();
        for change in changes {
            if let Some((index, exact)) = self.find(&roots, &children, &change.path) {
                *todo.entry(index).or_default() |= change.recursive && exact;
            }
        }
        // Recursive rescans first, so their descendants are skipped below.
        let mut todo: Vec<(u32, bool)> = todo.into_iter().collect();
        todo.sort_by_key(|&(_, recursive)| !recursive);

        let state = Walk {
            nodes: Mutex::new(std::mem::take(&mut self.nodes)),
            progress,
            dirs: AtomicU64::new(0),
        };
        let walk = &state;
        pool().scope(|scope| {
            for (index, recursive) in todo {
                let (at, path) = {
                    let nodes = walk.nodes.lock().unwrap();
                    let node = &nodes[index as usize];
                    if !node.alive {
                        continue;
                    }
                    let at = Place {
                        parent: node.parent,
                        group: node.group,
                        category: node.category,
                        tags: node.tags,
                        dev: 0,
                    };
                    (at, path_of(&nodes, index))
                };
                let (Ok(md), Ok(handle)) = (fs::metadata(&path), Dir::open(&path)) else {
                    kill(&mut walk.nodes.lock().unwrap(), &children, index, false);
                    continue;
                };
                let at = Place {
                    dev: md.dev(),
                    ..at
                };
                let listing = list(&handle, at.dev);
                drop(handle);

                let mut nodes = walk.nodes.lock().unwrap();
                let node = &mut nodes[index as usize];
                node.own = listing.own;
                node.largest = listing.largest;
                let existing: HashMap<&[u8], u32> = children
                    .of(index)
                    .iter()
                    .filter(|&&c| nodes[c as usize].alive)
                    .map(|&c| (&*nodes[c as usize].name, c))
                    .collect();
                let listed: HashSet<&[u8]> =
                    listing.subdirs.iter().map(|(n, _)| n.as_bytes()).collect();
                let gone: Vec<u32> = existing
                    .iter()
                    .filter(|(name, _)| recursive || !listed.contains(*name))
                    .map(|(_, &c)| c)
                    .collect();
                let new: Vec<_> = listing
                    .subdirs
                    .into_iter()
                    .filter(|(name, _)| recursive || !existing.contains_key(name.as_bytes()))
                    .collect();
                for child in gone {
                    kill(&mut nodes, &children, child, false);
                }
                drop(nodes);
                walk.children(scope, &path, index, new, &at);
            }
        });
        self.nodes = state.nodes.into_inner().unwrap();
    }

    /// The deepest known folder on `path`, and whether it is `path` itself.
    fn find(&self, roots: &[u32], children: &Children, path: &Path) -> Option<(u32, bool)> {
        let (mut index, rest) = roots.iter().find_map(|&r| {
            let root = Path::new(OsStr::from_bytes(&self.nodes[r as usize].name));
            path.strip_prefix(root).ok().map(|rest| (r, rest))
        })?;
        for part in rest.components() {
            let name = part.as_os_str().as_bytes();
            let child = children
                .of(index)
                .iter()
                .copied()
                .find(|&c| &*self.nodes[c as usize].name == name);
            match child {
                Some(child) => index = child,
                None => return Some((index, false)),
            }
        }
        Some((index, true))
    }

    pub fn totals(&self) -> [u64; 9] {
        let mut totals = [0; 9];
        for node in self.nodes.iter().filter(|n| n.alive) {
            totals[node.category as usize] += node.own;
        }
        totals
    }

    pub fn report(&self) -> Report {
        let mut report = Report::default();
        let mut groups: HashMap<u32, u64> = HashMap::new();
        for node in self.nodes.iter().filter(|n| n.alive) {
            let c = node.category as usize;
            if node.group != NONE {
                *groups.entry(node.group).or_default() += node.own;
            }
            if node.tags & SIMULATORS != 0 {
                report.simulators += node.own;
            }
            if node.tags & BUILD_OUTPUT != 0 {
                report.build_output += node.own;
            }
            if let Some((name, size)) = &node.largest
                && report.largest_file[c]
                    .as_ref()
                    .is_none_or(|(_, best)| size > best)
            {
                report.largest_file[c] = Some((name.to_string(), *size));
            }
        }
        for (group, bytes) in groups {
            let node = &self.nodes[group as usize];
            let slot = &mut report.largest_item[node.category as usize];
            if slot.as_ref().is_none_or(|(_, best)| bytes > *best) {
                *slot = Some((String::from_utf8_lossy(&node.name).into_owned(), bytes));
            }
        }
        report
    }
}

fn path_of(nodes: &[Node], mut index: u32) -> PathBuf {
    let mut parts = Vec::new();
    while index != NONE {
        let node = &nodes[index as usize];
        parts.push(OsStr::from_bytes(&node.name));
        index = node.parent;
    }
    parts.iter().rev().collect()
}

/// Marks `index` and everything below it as gone. With `keep_self`, only
/// the descendants go.
fn kill(nodes: &mut [Node], children: &Children, index: u32, keep_self: bool) {
    let mut stack = vec![index];
    while let Some(i) = stack.pop() {
        if i != index || !keep_self {
            nodes[i as usize].alive = false;
        }
        stack.extend(children.of(i));
    }
}

/// Each folder's subfolders as one flat array (built from parent links), so
/// indexing 400k folders takes milliseconds, not a map per folder.
struct Children {
    start: Vec<u32>,
    kids: Vec<u32>,
}

impl Children {
    fn new(nodes: &[Node]) -> Children {
        let mut start = vec![0u32; nodes.len() + 1];
        for node in nodes.iter().filter(|n| n.alive && n.parent != NONE) {
            start[node.parent as usize + 1] += 1;
        }
        for i in 0..nodes.len() {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut kids = vec![0u32; start[nodes.len()] as usize];
        for (i, node) in nodes.iter().enumerate() {
            if node.alive && node.parent != NONE {
                let slot = &mut fill[node.parent as usize];
                kids[*slot as usize] = i as u32;
                *slot += 1;
            }
        }
        Children { start, kids }
    }

    /// Subfolders of `index`; none for folders added after this was built.
    fn of(&self, index: u32) -> &[u32] {
        match (
            self.start.get(index as usize),
            self.start.get(index as usize + 1),
        ) {
            (Some(&a), Some(&b)) => &self.kids[a as usize..b as usize],
            _ => &[],
        }
    }
}

// Cache file: a flat list of live folders, parents before children.

const MAGIC: &[u8; 8] = b"BDSKTRE1";

fn cache_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/beautifuldisk/tree.bin")
}

impl Tree {
    pub fn save(&self, home: &Path) -> io::Result<()> {
        let path = cache_path(home);
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("tmp");
        let mut out = BufWriter::new(fs::File::create(&tmp)?);
        // Dead folders are dropped, so live ones get new, dense indices.
        let mut remap = vec![NONE; self.nodes.len()];
        let mut count = 0u32;
        for (i, node) in self.nodes.iter().enumerate() {
            if node.alive {
                remap[i] = count;
                count += 1;
            }
        }
        let index = |i: u32| if i == NONE { NONE } else { remap[i as usize] };
        out.write_all(MAGIC)?;
        out.write_all(&self.event_id.to_le_bytes())?;
        out.write_all(&count.to_le_bytes())?;
        for node in self.nodes.iter().filter(|n| n.alive) {
            out.write_all(&index(node.parent).to_le_bytes())?;
            out.write_all(&index(node.group).to_le_bytes())?;
            out.write_all(&[node.category as u8, node.tags])?;
            out.write_all(&node.own.to_le_bytes())?;
            out.write_all(&(node.name.len() as u32).to_le_bytes())?;
            out.write_all(&node.name)?;
            let (name, size) = node.largest.as_ref().map_or(("", 0), |(n, s)| (&**n, *s));
            out.write_all(&size.to_le_bytes())?;
            out.write_all(&(name.len() as u32).to_le_bytes())?;
            out.write_all(name.as_bytes())?;
        }
        out.into_inner()?.sync_all()?;
        fs::rename(tmp, path)
    }

    pub fn load(home: &Path) -> Option<Tree> {
        let mut bytes = Vec::new();
        fs::File::open(cache_path(home))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let mut r = Reader {
            bytes: &bytes,
            at: 0,
        };
        if r.take(8)? != MAGIC {
            return None;
        }
        let event_id = r.u64()?;
        let count = r.u32()? as usize;
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            let parent = r.u32()?;
            let group = r.u32()?;
            let category = *Category::ALL.get(r.u8()? as usize)?;
            let tags = r.u8()?;
            let own = r.u64()?;
            let len = r.u32()? as usize;
            let name = r.take(len)?.into();
            let size = r.u64()?;
            let len = r.u32()? as usize;
            let largest_name = std::str::from_utf8(r.take(len)?).ok()?;
            let parent_ok = parent == NONE || (parent as usize) < nodes.len();
            if !parent_ok {
                return None;
            }
            nodes.push(Node {
                parent,
                group,
                category,
                tags,
                alive: true,
                own,
                name,
                largest: (size > 0).then(|| (largest_name.into(), size)),
            });
        }
        Some(Tree { nodes, event_id })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}
