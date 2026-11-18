//! `image apfs-ls` and `image apfs-copy`: a folder's entries and a copy of a file or a folder, read from a raw
//! APFS image without mounting it. Only in this tool (imagetool.py has no such command).

use crate::apfs::fuse::{self, Backend, Mount};
use crate::apfs::{ApfsTree, Container, Pick, DT_DIR, DT_LNK, DT_REG};
use crate::{crash, err, say, Fail};
use oneios_common::job;
use std::path::Path;

/// `--newest` and `--checkpoint XID` as the state to read (neither: block 0's).
pub fn pick(newest: bool, checkpoint: Option<u64>) -> Pick {
    match checkpoint {
        Some(xid) => Pick::Xid(xid),
        None if newest => Pick::Newest,
        None => Pick::Block0,
    }
}

/// The volume `--volume N|NAME` names (default: the first), in the state `pick` names. Which checkpoint is read
/// is said on stderr when one was chosen (not for block 0's state, the default).
pub fn open(image: &str, volume: Option<&str>, pick: Pick) -> Result<ApfsTree, Fail> {
    let mut container = Container::open(image, pick)?;
    let (index, tree) = container.volume_named(volume)?;
    if let Some(opened) = tree.opened().filter(|_| pick != Pick::Block0) {
        eprintln!("{}", opened.text(image, index, &tree.name));
    }
    Ok(tree)
}

/// The volume as apfs-fuse mounts it: `--volume` must be a number there, or a name our reader can look up (as
/// block 0 has the volumes, else at the newest checkpoint). No `--volume`: the first volume, as our reader takes
/// it (place 0 when our reader cannot say).
fn mount(image: &str, volume: Option<&str>) -> Result<Mount, Fail> {
    let index = match volume {
        None => [Pick::Block0, Pick::Newest].into_iter().find_map(|pick| Container::open(image, pick).and_then(|c| c.volumes()).ok()).and_then(|v| v.first().copied()).unwrap_or(0),
        Some(v) => match v.parse::<usize>() {
            Ok(index) => index,
            // a name: its number, which our reader can say when it reads the container
            Err(_) => [Pick::Block0, Pick::Newest]
                .into_iter()
                .find_map(|pick| Container::open(image, pick).and_then(|mut c| c.volume_named(Some(v))).ok())
                .map(|(index, _)| index)
                .ok_or_else(|| err!("--apfs fuse: no volume named {v} that our reader can look up; give the volume's number"))?,
        },
    };
    Mount::new(image, index)
}

/// Our reader, apfs-fuse, or ours first and apfs-fuse when ours refuses (`--apfs auto`): what `own` gives, else
/// what `fuse` gives on the mounted volume's root.
fn with_backend<T>(image: &str, volume: Option<&str>, backend: Backend, own: impl FnOnce() -> Result<T, Fail>, fuse: impl FnOnce(&Path) -> Result<T, Fail>) -> Result<T, Fail> {
    // ONEIOS_APFS_TEST_REFUSE: for the tests of the fallback, our reader refuses as it would an image it cannot read
    let own = || if std::env::var_os("ONEIOS_APFS_TEST_REFUSE").is_some() { Err(err!("{image}: refused for a test (ONEIOS_APFS_TEST_REFUSE)")) } else { own() };
    match backend {
        Backend::Own => own(),
        Backend::Fuse => fuse(&mount(image, volume)?.root()),
        Backend::Auto => match own() {
            Ok(done) => Ok(done),
            Err(e) if fuse::available() => {
                eprintln!("{e}\nour APFS reader refused; reading through apfs-fuse instead");
                fuse(&mount(image, volume)?.root())
            }
            Err(e) => Err(e),
        },
    }
}

pub fn ls(image: &str, path: &str, volume: Option<&str>, pick: Pick, backend: Backend) -> Result<(), Fail> {
    let own = || -> Result<String, Fail> {
        let tree = open(image, volume, pick)?;
        let dir = tree.find(path)?.ok_or_else(|| err!("{image}: no /{path} in the volume"))?;
        let mut entries = tree.list(dir)?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut text = String::new();
        for e in &entries {
            let st = tree.stat(e.id)?;
            text.push_str(&format!("{:06o} {:>5} {:>5} {:>12} {}\n", st.mode, st.uid, st.gid, st.size, e.name));
        }
        Ok(text)
    };
    let mounted = |root: &Path| -> Result<String, Fail> {
        use std::os::unix::fs::MetadataExt;
        let dir = root.join(path.trim_matches('/'));
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(&dir).map_err(|_| err!("{image}: no /{path} in the volume"))?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        let mut text = String::new();
        for name in names {
            let st = std::fs::symlink_metadata(dir.join(&name)).map_err(|e| err!("{}: {e}", dir.join(&name).display()))?;
            // a folder's size is the readers' own business (ours: 0, apfs-fuse: a count): 0 for both
            let size = if st.is_dir() { 0 } else { st.size() };
            text.push_str(&format!("{:06o} {:>5} {:>5} {:>12} {}\n", st.mode(), st.uid(), st.gid(), size, name.to_string_lossy()));
        }
        Ok(text)
    };
    let text = with_backend(image, volume, backend, own, mounted)?;
    say(text.trim_end_matches('\n'))
}

struct Copied {
    files: u64,
    bytes: u64,
    /// files that were kept compressed in the image and are written plain
    compressed: u64,
    /// compressed files that could not be decoded
    skipped: u64,
}

fn copy_one(tree: &ApfsTree, id: u64, kind: u16, dst: &Path, done: &mut Copied) -> Result<(), Fail> {
    let bad = |e: std::io::Error| crash!("{}: {e}", dst.display());
    match kind {
        DT_DIR => {
            std::fs::create_dir_all(dst).map_err(bad)?;
            for e in tree.list(id)? {
                if e.name.contains('/') || e.name == "." || e.name == ".." {
                    return Err(err!("a name in the volume's records is not a file name: {:?}", e.name));
                }
                copy_one(tree, e.id, e.kind, &dst.join(&e.name), done)?;
            }
        }
        DT_REG => {
            let st = tree.stat(id)?;
            let data = match tree.read_file(id) {
                Ok(data) => data,
                // a compressed file this reader cannot decode (LZBITMAP) is said, counted and left out
                Err(e) if ApfsTree::is_compressed(&st) => {
                    eprintln!("{}: {e}", dst.display());
                    done.skipped += 1;
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            if ApfsTree::is_compressed(&st) {
                done.compressed += 1;
            }
            std::fs::write(dst, &data).map_err(bad)?;
            done.files += 1;
            done.bytes += data.len() as u64;
        }
        DT_LNK => {
            if let Some(target) = tree.xattr(id, "com.apple.fs.symlink")? {
                let target = String::from_utf8_lossy(&target[..target.iter().position(|b| *b == 0).unwrap_or(target.len())]).into_owned();
                let _ = std::fs::remove_file(dst);
                std::os::unix::fs::symlink(target, dst).map_err(bad)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// One entry of the tree that is copied, in the order `copy_one` meets them.
struct Item {
    id: u64,
    kind: u16,
    dst: std::path::PathBuf,
    /// A file's size (a compressed file's: its plain size).
    size: u64,
}

/// The entries below `id`, in `copy_one`'s order: a folder, then what it holds, entry by entry. The folders are
/// listed and the files' sizes read in parallel; the order of the result does not depend on that.
fn scan(tree: &ApfsTree, id: u64, kind: u16, dst: &Path) -> Result<Vec<Item>, Fail> {
    use rayon::prelude::*;
    job_check()?;
    match kind {
        DT_DIR => {
            let entries = tree.list(id)?;
            if let Some(e) = entries.iter().find(|e| e.name.contains('/') || e.name == "." || e.name == "..") {
                return Err(err!("a name in the volume's records is not a file name: {:?}", e.name));
            }
            let below: Vec<Vec<Item>> = entries.par_iter().map(|e| scan(tree, e.id, e.kind, &dst.join(&e.name))).collect::<Result<_, Fail>>()?;
            let mut out = vec![Item { id, kind, dst: dst.to_path_buf(), size: 0 }];
            out.extend(below.into_iter().flatten());
            Ok(out)
        }
        DT_REG => Ok(vec![Item { id, kind, dst: dst.to_path_buf(), size: tree.stat(id)?.size }]),
        DT_LNK => Ok(vec![Item { id, kind, dst: dst.to_path_buf(), size: 0 }]),
        _ => Ok(Vec::new()),
    }
}

/// The running request's check (`serve`: a cancel, the heat and battery limits); nothing is asked of a command
/// that runs alone.
fn job_check() -> Result<(), Fail> {
    job::check().map_err(|stop| crash!("{stop}"))
}

/// File contents held in memory at one time, over all threads: a file is read whole before it is written, and
/// the cache files of an iOS volume are hundreds of megabytes each. A file larger than this is copied alone.
const IN_MEMORY_MAX: u64 = 1 << 30;

/// A count of bytes that threads take from and give back to.
struct Budget {
    left: std::sync::Mutex<u64>,
    freed: std::sync::Condvar,
}

impl Budget {
    fn take(&self, bytes: u64) -> u64 {
        let want = bytes.min(IN_MEMORY_MAX);
        let mut left = self.left.lock().unwrap();
        while *left < want {
            left = self.freed.wait(left).unwrap();
        }
        *left -= want;
        want
    }
    fn give(&self, bytes: u64) {
        *self.left.lock().unwrap() += bytes;
        self.freed.notify_all();
    }
}

/// `copy_one`'s work in every thread of the pool: the folders first, in order; then the files, each read and
/// written by one thread; then the links. The same files, bytes, counts and messages as `copy_one` gives (a
/// message about a file that was left out comes at the end, in `copy_one`'s order). `ONEIOS_APFS_COPY=serial`
/// runs `copy_one` instead, as does a volume that names one path twice (there the last one wins, in order).
fn copy_tree(tree: &ApfsTree, id: u64, kind: u16, dst: &Path, done: &mut Copied) -> Result<(), Fail> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    if std::env::var("ONEIOS_APFS_COPY").is_ok_and(|v| v == "serial") {
        return copy_one(tree, id, kind, dst, done);
    }
    // (ONEIOS_IMAGE_TIMING=1: each part's time on stderr)
    let mut laps = crate::Laps::start();
    job::step("scan");
    let items = scan(tree, id, kind, dst)?;
    laps.lap("apfs-copy scan");
    {
        let mut seen = std::collections::HashSet::new();
        if items.iter().filter(|i| i.kind != DT_DIR).any(|i| !seen.insert(&i.dst)) {
            return copy_one(tree, id, kind, dst, done);
        }
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    let files = items.iter().filter(|i| i.kind == DT_REG).count() as u64;
    if let Some(space) = SPACE_CHECK.get() {
        space(dst, total)?;
    }
    let bad = |dst: &Path, e: std::io::Error| crash!("{}: {e}", dst.display());
    for item in items.iter().filter(|i| i.kind == DT_DIR) {
        std::fs::create_dir_all(&item.dst).map_err(|e| bad(&item.dst, e))?;
    }
    let progress = job::begin("copy", "bytes", total).items(files);
    let budget = Budget { left: std::sync::Mutex::new(IN_MEMORY_MAX), freed: std::sync::Condvar::new() };
    let failed = AtomicBool::new(false);
    let (bytes, compressed) = (AtomicU64::new(0), AtomicU64::new(0));
    // (index in copy_one's order, the message) of each compressed file that could not be decoded
    let skipped = std::sync::Mutex::new(Vec::<(usize, String)>::new());
    let result: Result<(), (usize, Fail)> = items
        .par_iter()
        .enumerate()
        .filter(|(_, item)| item.kind == DT_REG)
        .try_for_each(|(at, item)| -> Result<(), (usize, Fail)> {
            if failed.load(Ordering::Relaxed) {
                return Ok(());
            }
            let one = || -> Result<(), Fail> {
                job_check()?;
                let st = tree.stat(item.id)?;
                let held = budget.take(st.size);
                let read = tree.read_file(item.id);
                let written = match read {
                    Ok(data) => {
                        if ApfsTree::is_compressed(&st) {
                            compressed.fetch_add(1, Ordering::Relaxed);
                        }
                        let n = data.len() as u64;
                        std::fs::write(&item.dst, &data).map(|()| n).map_err(|e| bad(&item.dst, e))
                    }
                    // a compressed file this reader cannot decode is said, counted and left out
                    Err(e) if ApfsTree::is_compressed(&st) => {
                        skipped.lock().unwrap().push((at, format!("{}: {e}", item.dst.display())));
                        budget.give(held);
                        progress.add(item.size);
                        progress.item();
                        return Ok(());
                    }
                    Err(e) => Err(e),
                };
                budget.give(held);
                bytes.fetch_add(written?, Ordering::Relaxed);
                progress.add(item.size);
                progress.item();
                Ok(())
            };
            one().map_err(|e| {
                failed.store(true, Ordering::Relaxed);
                (at, e)
            })
        });
    job::end();
    laps.lap("apfs-copy files");
    let mut skipped = skipped.into_inner().unwrap();
    skipped.sort();
    for (_, message) in &skipped {
        eprintln!("{message}");
    }
    result.map_err(|(_, e)| e)?;
    for item in items.iter().filter(|i| i.kind == DT_LNK) {
        job_check()?;
        if let Some(target) = tree.xattr(item.id, "com.apple.fs.symlink")? {
            let target = String::from_utf8_lossy(&target[..target.iter().position(|b| *b == 0).unwrap_or(target.len())]).into_owned();
            let _ = std::fs::remove_file(&item.dst);
            std::os::unix::fs::symlink(target, &item.dst).map_err(|e| bad(&item.dst, e))?;
        }
    }
    done.skipped += skipped.len() as u64;
    done.files += files - skipped.len() as u64;
    done.bytes += bytes.into_inner();
    done.compressed += compressed.into_inner();
    Ok(())
}

/// `serve`'s check that the folder a copy writes to has room for it, asked once the copy knows its size.
pub type SpaceCheck = Box<dyn Fn(&Path, u64) -> Result<(), Fail> + Send + Sync>;
pub static SPACE_CHECK: std::sync::OnceLock<SpaceCheck> = std::sync::OnceLock::new();

/// What a copy did, for `serve`'s result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CopyResult {
    pub files: u64,
    pub bytes: u64,
    pub compressed: u64,
    pub skipped: u64,
}

/// A file, a link or a folder of a mounted volume to `dst`, as `copy_one` writes them.
fn copy_mounted(src: &Path, dst: &Path, done: &mut Copied) -> Result<(), Fail> {
    let bad = |e: std::io::Error| crash!("{}: {e}", dst.display());
    let kind = std::fs::symlink_metadata(src).map_err(|e| err!("{}: {e}", src.display()))?.file_type();
    if kind.is_dir() {
        std::fs::create_dir_all(dst).map_err(bad)?;
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(src).map_err(|e| err!("{}: {e}", src.display()))?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for name in names {
            copy_mounted(&src.join(&name), &dst.join(&name), done)?;
        }
    } else if kind.is_file() {
        done.bytes += std::fs::copy(src, dst).map_err(|e| crash!("{} -> {}: {e}", src.display(), dst.display()))?;
        done.files += 1;
    } else if kind.is_symlink() {
        let _ = std::fs::remove_file(dst);
        std::os::unix::fs::symlink(std::fs::read_link(src).map_err(bad)?, dst).map_err(bad)?;
    }
    Ok(())
}

/// Our reader's copy of `src` to `dst`: the folders, then the files in every thread of the pool, then the links.
fn copy_own(image: &str, src: &str, dst: &str, volume: Option<&str>, pick: Pick) -> Result<Copied, Fail> {
    let tree = open(image, volume, pick)?;
    let id = tree.find(src)?.ok_or_else(|| err!("{image}: no /{} in the volume", src.trim_matches('/')))?;
    let kind = match tree.stat(id)?.mode & 0o170000 {
        0o040000 => DT_DIR,
        0o100000 => DT_REG,
        0o120000 => DT_LNK,
        other => return Err(err!("{image}: /{} is not a file, a folder or a link (mode {other:o})", src.trim_matches('/'))),
    };
    let mut done = Copied { files: 0, bytes: 0, compressed: 0, skipped: 0 };
    copy_tree(&tree, id, kind, Path::new(dst), &mut done)?;
    Ok(done)
}

/// `image apfs-copy` for `serve`: our reader only (it starts no apfs-fuse), and the counts instead of a line.
pub fn copy_to(image: &str, src: &str, dst: &str, volume: Option<&str>, pick: Pick) -> Result<CopyResult, Fail> {
    let done = copy_own(image, src, dst, volume, pick)?;
    Ok(CopyResult { files: done.files, bytes: done.bytes, compressed: done.compressed, skipped: done.skipped })
}

/// `image apfs-ls` for `serve`: (mode, uid, gid, size, name) of each entry, by name.
pub fn list(image: &str, path: &str, volume: Option<&str>, pick: Pick) -> Result<Vec<(u16, u32, u32, u64, String)>, Fail> {
    let tree = open(image, volume, pick)?;
    let dir = tree.find(path)?.ok_or_else(|| err!("{image}: no /{} in the volume", path.trim_matches('/')))?;
    let mut entries = tree.list(dir)?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
        .into_iter()
        .map(|e| {
            let st = tree.stat(e.id)?;
            Ok((st.mode, st.uid, st.gid, st.size, e.name))
        })
        .collect()
}

pub fn copy(image: &str, src: &str, dst: &str, volume: Option<&str>, pick: Pick, backend: Backend) -> Result<(), Fail> {
    let own = || copy_own(image, src, dst, volume, pick);
    let mounted = |root: &Path| -> Result<Copied, Fail> {
        let from = root.join(src.trim_matches('/'));
        if std::fs::symlink_metadata(&from).is_err() {
            return Err(err!("{image}: no /{} in the volume", src.trim_matches('/')));
        }
        let mut done = Copied { files: 0, bytes: 0, compressed: 0, skipped: 0 };
        copy_mounted(&from, Path::new(dst), &mut done)?;
        Ok(done)
    };
    let done = with_backend(image, volume, backend, own, mounted)?;
    let was = if done.compressed == 0 { String::new() } else { format!(" ({} of them were kept compressed)", done.compressed) };
    let more = if done.skipped == 0 { String::new() } else { format!(", skipped {} compressed files", done.skipped) };
    say(&format!("copied {} files, {} bytes{was}{more}", done.files, done.bytes))
}
