//! `image apfs-copy` in threads against the copy one entry after the other (`ONEIOS_APFS_COPY=serial`): the same
//! files, bytes, links and counts from an image made by tests/ioslog/mkapfs.py, with one thread and with several;
//! a copy that is stopped (the running request's cancel, `oneios_common::job`) ends with an error. No Apple file.
//! Needs python3 for mkapfs.py (the stand-in images' maker, as tests/apfs_files.rs) and is skipped, with a line on
//! stderr, where there is none.
//!
//! One test function: it sets an environment variable and the process's stop flag, which other tests of the same
//! binary would see.

use oneios_image::apfs::Pick;
use oneios_image::apfs_tools::{copy_to, CopyResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oneios-apfs-copy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| ((i / 7) as u8) ^ (i as u8).wrapping_mul(31) ^ seed).collect()
}

/// Every entry below `dir`: its path, and its bytes or its link's target or "dir".
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(top: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(at).unwrap().flatten() {
            let path = e.path();
            let rel = path.strip_prefix(top).unwrap().to_string_lossy().into_owned();
            let kind = e.file_type().unwrap();
            if kind.is_symlink() {
                out.insert(rel, format!("-> {}", std::fs::read_link(&path).unwrap().display()).into_bytes());
            } else if kind.is_dir() {
                out.insert(format!("{rel}/"), b"dir".to_vec());
                walk(top, &path, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn the_copy_in_threads_is_the_copy_one_by_one() {
    let mkapfs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/ioslog/mkapfs.py");
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3 for {}", mkapfs.display());
        return;
    }
    let dir = scratch("made");
    let src = dir.join("src");
    for folder in ["System/Library/Caches", "System/Library/Frameworks/A.framework", "private/var/empty", "usr/lib"] {
        std::fs::create_dir_all(src.join(folder)).unwrap();
    }
    std::fs::write(src.join("System/Library/Caches/big"), pattern(900_000, 2)).unwrap();
    std::fs::write(src.join("System/Library/Caches/none"), b"").unwrap();
    std::fs::write(src.join("usr/lib/one"), pattern(5000, 1)).unwrap();
    for i in 0..600 {
        std::fs::write(src.join(format!("System/Library/Frameworks/A.framework/f{i:03}")), pattern(100 + i * 37, i as u8)).unwrap();
    }
    std::os::unix::fs::symlink("Library/Caches", src.join("System/link")).unwrap();
    std::os::unix::fs::symlink("/usr/lib/one", src.join("usr/lib/abs-link")).unwrap();
    let image = dir.join("t.img");
    let made = Command::new("python3").arg("-I").arg(&mkapfs).arg(&image).arg(format!("Data={}", src.display())).output().unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let image = image.to_str().unwrap();
    let want = tree(&src);
    let files = want.values().filter(|v| v.as_slice() != b"dir" && !v.starts_with(b"-> ")).count() as u64;
    let bytes: u64 = want.values().filter(|v| v.as_slice() != b"dir" && !v.starts_with(b"-> ")).map(|v| v.len() as u64).sum();
    let counts = CopyResult { files, bytes, compressed: 0, skipped: 0 };

    let copy = |to: &str| copy_to(image, "/", dir.join(to).to_str().unwrap(), None, Pick::Block0);
    // one entry after the other: the path that was there before
    std::env::set_var("ONEIOS_APFS_COPY", "serial");
    assert_eq!(copy("serial").unwrap(), counts);
    std::env::remove_var("ONEIOS_APFS_COPY");
    assert_eq!(tree(&dir.join("serial")), want, "the copy one by one is the folder the image was made from");
    // in threads: one, and several
    for threads in [1, 2, 8] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        let name = format!("threads-{threads}");
        assert_eq!(pool.install(|| copy(&name)).unwrap(), counts, "{threads} threads");
        assert_eq!(tree(&dir.join(&name)), want, "{threads} threads");
    }
    // a part of the volume, a single file, a single link
    let part = |from: &str, to: &str| copy_to(image, from, dir.join(to).to_str().unwrap(), None, Pick::Block0).unwrap();
    assert_eq!(part("/System/Library/Frameworks", "frameworks").files, 600);
    assert_eq!(part("usr/lib/one", "one-file"), CopyResult { files: 1, bytes: 5000, compressed: 0, skipped: 0 });
    assert_eq!(std::fs::read(dir.join("one-file")).unwrap(), pattern(5000, 1));
    assert_eq!(part("System/link", "one-link").files, 0);
    assert_eq!(std::fs::read_link(dir.join("one-link")).unwrap(), Path::new("Library/Caches"));
    assert!(copy_to(image, "/no/such", dir.join("x").to_str().unwrap(), None, Pick::Block0).unwrap_err().to_string().contains("no /no/such in the volume"));

    // a stop of the running request ends the copy with an error (the caller removes what was written)
    oneios_common::job::cancel();
    let stopped = copy("stopped").unwrap_err().to_string();
    oneios_common::job::clear_cancel();
    assert!(stopped.contains("stopped"), "{stopped}");
    let _ = std::fs::remove_dir_all(&dir);
}
