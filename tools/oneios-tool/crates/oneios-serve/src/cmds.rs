//! The commands of `oneios-tool serve` (README "serve", "The commands"): each reads its keys, checks what it may
//! (the paths it writes, the room it needs), runs the tool's own code in the request's threads and says the result.
//! A command's steps are listed here, in order, with a line for the screen; the code that does a step names it
//! (`job::step`, `job::begin`).
//!
//! Nothing here starts a program: the commands that would (the adb forms of `phone`, `app`, `cycle`, the apfs-fuse
//! reader, the prelink's `--x18`) are not commands of `serve`.

use crate::proto::{Done, Fail};
use crate::{size_text, ReqSink, Rq, NOW, SPACE_RESERVE};
use oneios_common::job;
use oneios_image::apfs::Pick;
use rayon::prelude::*;
use serde_json::json;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Steps = &'static [(&'static str, &'static str)];

pub struct Spec {
    pub name: &'static str,
    /// Checked for heat, battery and an open session (README "Threads, nice, heat").
    pub heavy: bool,
    /// A `pause` request holds its transfer (README "Downloads on a mobile network"): the network requests.
    pub pausable: bool,
    pub steps: Steps,
    pub run: fn(&Rq) -> Result<Done, Fail>,
}

pub static ALL: &[Spec] = &[
    Spec { name: "wait", heavy: false, pausable: false, steps: &[("wait", "Waiting")], run: wait },
    Spec { name: "download", heavy: true, pausable: true, steps: &[("download", "Downloading"), ("check", "Checking the file"), ("place", "Putting the file in place")], run: download },
    Spec { name: "zip-ls", heavy: false, pausable: false, steps: &[], run: zip_ls },
    Spec { name: "zip-get", heavy: true, pausable: true, steps: &[("directory", "Reading the archive's directory"), ("download", "Downloading and checking the members")], run: zip_get },
    Spec { name: "fetch", heavy: true, pausable: true, steps: &[("directory", "Reading the IPSW's directory"), ("download", "Downloading and checking the members")], run: fetch },
    Spec { name: "im4p", heavy: true, pausable: false, steps: &[("decode", "Decoding")], run: im4p },
    Spec { name: "receive", heavy: true, pausable: false, steps: &[("copy", "Copying the file"), ("check", "Checking the file"), ("place", "Putting the file in place")], run: receive },
    Spec { name: "sums", heavy: true, pausable: false, steps: &[("scan", "Reading the list"), ("hash", "Checking the files")], run: sums },
    Spec { name: "hashes", heavy: true, pausable: false, steps: &[("hash", "Hashing the file")], run: hashes },
    Spec { name: "verify", heavy: true, pausable: false, steps: &[("verify", "Checking the image set")], run: verify },
    Spec { name: "apfs-ls", heavy: false, pausable: false, steps: &[], run: apfs_ls },
    Spec { name: "apfs-copy", heavy: true, pausable: false, steps: &[("scan", "Reading the volume"), ("copy", "Copying the files"), ("place", "Putting the folder in place")], run: apfs_copy },
    Spec { name: "tweaks", heavy: true, pausable: false, steps: &[], run: tweaks },
    Spec { name: "prelink", heavy: true, pausable: false, steps: &oneios_prelink::build::STEPS, run: prelink },
    Spec { name: "stubs-check", heavy: true, pausable: false, steps: &[("check", "Checking the stubs")], run: stubs_check },
    Spec { name: "package-check", heavy: true, pausable: false, steps: &[("check", "Checking the runtime's files")], run: package_check },
    Spec { name: "ioslog-boots", heavy: true, pausable: false, steps: &[("read", "Reading iOS's log")], run: ioslog_boots },
    Spec { name: "ioslog-show", heavy: true, pausable: false, steps: &[("read", "Reading iOS's log")], run: ioslog_show },
    Spec { name: "log-list", heavy: false, pausable: false, steps: &[], run: log_list },
    Spec { name: "log", heavy: false, pausable: false, steps: &[], run: log },
    Spec { name: "cpu", heavy: false, pausable: false, steps: &[], run: cpu },
];

pub fn find(name: &str) -> Option<&'static Spec> {
    ALL.iter().find(|spec| spec.name == name)
}

/// The room check of a copy that learns its size only inside the image crate: asked through that crate's hook, for
/// the request that runs.
pub fn hooks() {
    let _ = oneios_image::apfs_tools::SPACE_CHECK.set(Box::new(|path, need| {
        let Some(sink) = NOW.lock().unwrap().clone() else { return Ok(()) };
        space_for(&sink, path, need).map_err(|fail| {
            *sink.refusal.lock().unwrap() = Some(fail);
            oneios_image::Fail::Error("not enough free space".into())
        })
    }));
}

/// The `space` event of the request, and the refusal when the room is short.
pub fn space_for(sink: &ReqSink, path: &Path, need: u64) -> Result<(), Fail> {
    let Some((folder, free)) = crate::free_bytes(path) else { return Ok(()) };
    sink.out.send(&crate::proto::Ev::of("space", &sink.id).f("path", folder.to_string_lossy()).f("need", need).f("free", free).f("reserve", SPACE_RESERVE));
    if free < need.saturating_add(SPACE_RESERVE) {
        return Err(Fail::refused(
            "space",
            format!("Not enough free space in {}: {} are needed and {} must stay free, {} are free", folder.display(), size_text(need), size_text(SPACE_RESERVE), size_text(free)),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- helpers

fn io(what: &str, e: std::io::Error) -> Fail {
    Fail::io(format!("{what}: {e}"))
}

/// An error of the image tool: its own refusals of a path that is not there, else a failure.
fn image_fail(e: oneios_image::Fail) -> Fail {
    let text = e.to_string();
    match e {
        oneios_image::Fail::Error(_) if text.contains(": no /") => Fail::refused("not-found", text),
        oneios_image::Fail::Error(_) => Fail::failed(text),
        _ => Fail::io(text),
    }
}

/// The running request's stop as an error (the runner says which stop it was).
fn stopped(stop: job::Stop) -> Fail {
    Fail::failed(stop.to_string())
}

fn check() -> Result<(), Fail> {
    job::check().map_err(stopped)
}

fn remove(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// The output must not be there; the folder it goes into is made.
fn fresh(path: &str) -> Result<(), Fail> {
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(Fail::refused("exists", format!("{path} is there already")));
    }
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
    }
    Ok(())
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn pick(rq: &Rq) -> Result<(Option<String>, Pick), Fail> {
    let volume = rq.req.opt_str("volume")?;
    let (newest, checkpoint) = (rq.req.flag("newest")?, rq.req.opt_u64("checkpoint")?);
    if newest && checkpoint.is_some() {
        return Err(Fail::usage(format!("{}: \"newest\" or \"checkpoint\", not both", rq.req.cmd)));
    }
    Ok((volume, oneios_image::apfs_tools::pick(newest, checkpoint)))
}

/// The sizes of a folder's files, summed (the stage's own folders: not /sys, not /proc).
fn tree_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(kind) if kind.is_dir() => tree_bytes(&e.path()),
            Ok(kind) if kind.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// The files of a folder, as paths below it.
fn tree_files(dir: &Path, below: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if below.is_empty() { name } else { format!("{below}/{name}") };
        match e.file_type() {
            Ok(kind) if kind.is_dir() => tree_files(&e.path(), &rel, out),
            Ok(_) => out.push(rel),
            Err(_) => {}
        }
    }
}

// ---------------------------------------------------------------- wait

/// For the app's own tests of progress, cancel and the queue: counts to `seconds` in the request's threads, with a
/// temporary file it removes.
fn wait(rq: &Rq) -> Result<Done, Fail> {
    let seconds = rq.req.opt_f64("seconds")?.unwrap_or(1.0);
    if !(0.0..=3600.0).contains(&seconds) {
        return Err(Fail::usage("wait: \"seconds\" must be 0 to 3600"));
    }
    let _ = rq.req.flag("heavy")?;
    let file = match rq.req.opt_str("file")? {
        Some(_) => Some(rq.write_path("file")?),
        None => None,
    };
    rq.req.done()?;
    let guard = match &file {
        Some(file) => {
            fresh(file)?;
            let guard = job::temp(file);
            std::fs::write(file, b"oneios-tool serve: a wait request's temporary file\n").map_err(|e| io(file, e))?;
            Some(guard)
        }
        None => None,
    };
    let total = (seconds * 1000.0) as u64;
    let progress = job::begin("wait", "ms", total);
    let started = Instant::now();
    let seen = Mutex::new(HashSet::new());
    (0..rq.jobs)
        .into_par_iter()
        .try_for_each(|_| -> Result<(), job::Stop> {
            seen.lock().unwrap().insert(std::thread::current().id());
            loop {
                let done = started.elapsed().as_millis() as u64;
                progress.set(done.min(total));
                if done >= total {
                    return Ok(());
                }
                job::sleep(Duration::from_millis((total - done).min(20)))?;
            }
        })
        .map_err(stopped)?;
    job::end();
    drop(guard);
    let (nice, realtime) = job::nice_now();
    let threads = seen.into_inner().unwrap().len();
    Ok(Done::new(format!("waited {seconds} s in {threads} thread{}", if threads == 1 { "" } else { "s" })).f("threads", threads).f("nice", nice).f("realtime", realtime))
}

// ---------------------------------------------------------------- downloads

fn source_of(rq: &Rq, key: &str) -> Result<String, Fail> {
    let source = rq.req.str(key)?;
    if source.starts_with("https://") || source.starts_with("http://127.0.0.1") || source.starts_with("http://localhost") {
        return Ok(source);
    }
    if source.contains("://") {
        return Err(Fail::usage(format!("{}: \"{key}\" must be an https URL or a file's absolute path", rq.req.cmd)));
    }
    if !Path::new(&source).is_absolute() || !Path::new(&source).is_file() {
        return Err(Fail::refused("not-found", format!("{source} is not a file (an https URL or a file's absolute path)")));
    }
    Ok(source)
}

/// The chunk lines of a fetch (`NAME: 12% (..)`) are the progress events' numbers: not sent as lines.
fn quiet(line: String) {
    if !line.contains("% (") {
        println!("{line}");
    }
}

/// The keys every network request takes: `give_up_s` (seconds without a byte after which it gives up) and, where
/// a file is fetched, `connections` (never more than the request's threads). Returns the connections.
fn net_keys(rq: &Rq, connections: bool) -> Result<usize, Fail> {
    use oneios_image::transfer::{begin_request, CONNECTIONS_MAX, GIVE_UP_DEFAULT};
    let give_up = match rq.req.opt_u64("give_up_s")? {
        None => GIVE_UP_DEFAULT,
        Some(seconds) if (1..=86_400).contains(&seconds) => seconds,
        Some(_) => return Err(Fail::usage(format!("{}: \"give_up_s\" must be 1 to 86400", rq.req.cmd))),
    };
    // (a key of the commands that fetch a file only: elsewhere it is an unknown key)
    let count = match if connections { rq.req.opt_u64("connections")? } else { None } {
        None => CONNECTIONS_MAX,
        Some(n) if (1..=CONNECTIONS_MAX as u64).contains(&n) => n as usize,
        Some(_) => return Err(Fail::usage(format!("{}: \"connections\" must be 1 to {CONNECTIONS_MAX}", rq.req.cmd))),
    };
    begin_request(give_up);
    Ok(count.min(rq.jobs))
}

/// A transfer's failure, with the network's own code when it was the network's doing (`network`: it gave up;
/// `changed`: the server's file is another one now).
fn net_fail(text: String) -> Fail {
    match oneios_image::transfer::take_failure() {
        Some(code) => Fail::Failed(code, text),
        None => Fail::failed(text),
    }
}

/// The SHA-256 of a part's unbroken beginning, read from the part as the download fills it (SHA-256 has one order
/// and the pieces arrive in any, so the hash follows the lowest piece that is not whole). `hashed` is how far it
/// is, for the `check` step. Returns how many bytes it hashed, and their sum.
fn hash_while_it_arrives(part: &str, frontier: &oneios_image::transfer::Frontier, hashed: &AtomicU64) -> Result<(u64, String), Fail> {
    use sha2::Digest;
    use std::os::unix::fs::FileExt;
    let (mut hash, mut block, mut at, mut file) = (sha2::Sha256::new(), vec![0u8; 1 << 20], 0u64, None);
    while let Some(reach) = frontier.wait(at) {
        let file = match &file {
            Some(file) => file,
            None => file.insert(std::fs::File::open(part).map_err(|e| io(part, e))?),
        };
        while at < reach {
            check()?;
            let n = block.len().min((reach - at) as usize);
            file.read_exact_at(&mut block[..n], at).map_err(|e| io(part, e))?;
            hash.update(&block[..n]);
            at += n as u64;
            hashed.store(at, Ordering::Relaxed);
        }
    }
    Ok((at, oneios_common::hex(&hash.finalize())))
}

fn download(rq: &Rq) -> Result<Done, Fail> {
    let url = source_of(rq, "url")?;
    let out = rq.write_path("out")?;
    let want = rq.req.str("sha256")?;
    let (bytes, resume) = (rq.req.opt_u64("bytes")?, rq.req.flag("resume")?);
    let connections = net_keys(rq, true)?;
    rq.req.done()?;
    if !is_sha256(&want) {
        return Err(Fail::usage("download: \"sha256\" must be 64 lower-case hex digits"));
    }
    let part = format!("{out}.part");
    // the part, its state file (and that one's own temporary name), and the chunk list of the tool before it
    let names = [part.clone(), format!("{part}.state"), format!("{part}.state.tmp"), format!("{part}.done")];
    if Path::new(&out).is_file() && oneios_image::sha256_of(&out).map_err(image_fail)? == want {
        names.iter().for_each(|name| remove(Path::new(name)));
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        return Ok(Done::new(format!("{out} is there already, with this SHA-256")).f("out", out.as_str()).f("bytes", size).f("sha256", want));
    }
    if let Some(parent) = Path::new(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
    }
    // With "resume" the part and its state are no temporary files: whatever ends the request (also the process's
    // end that does not wait for a thread inside a name lookup, which removes the temporary files) leaves them
    // for the next run. Without it they go when the request ends early.
    let guards: Vec<job::Temp> = if resume { Vec::new() } else { names.iter().map(job::temp).collect() };
    if !resume {
        names.iter().for_each(|name| remove(Path::new(name)));
    }
    // (true: the part is not what was asked for, and is not kept whatever the request says)
    let mut wrong = false;
    let done = download_part(rq, &url, &out, &part, &want, bytes, connections, &mut wrong);
    match &done {
        Ok(_) => {
            guards.into_iter().for_each(job::Temp::keep);
            names[1..].iter().for_each(|name| remove(Path::new(name)));
        }
        Err(_) if wrong => {
            drop(guards);
            names.iter().for_each(|name| remove(Path::new(name)));
        }
        // (a download that was paused when the process ended: the user had it on hold)
        Err(_) if job::held_at_the_end() => guards.into_iter().for_each(job::Temp::keep),
        Err(_) => {}
    }
    done
}

/// The download into `part`, its SHA-256 and the file's name. An error leaves the part to the caller.
#[allow(clippy::too_many_arguments)]
fn download_part(rq: &Rq, url: &str, out: &str, part: &str, want: &str, bytes: Option<u64>, connections: usize, wrong: &mut bool) -> Result<Done, Fail> {
    use oneios_image::transfer::{self, Frontier};
    let refusal: Mutex<Option<Fail>> = Mutex::new(None);
    let begin = |size: u64| -> Result<(), String> {
        // (the room: what the part does not hold yet)
        let have = std::fs::metadata(part).map(|m| (m.blocks() * 512).min(size)).unwrap_or(0);
        let ok = match bytes {
            Some(bytes) if bytes != size => Err(Fail::refused("mismatch", format!("{url} has {size} bytes, {bytes} were expected"))),
            _ => rq.space(Path::new(out), size - have),
        };
        job::begin("download", "bytes", size);
        ok.map_err(|fail| {
            let text = match &fail {
                Fail::Refused(_, text) | Fail::Failed(_, text) => text.clone(),
            };
            *refusal.lock().unwrap() = Some(fail);
            text
        })
    };
    let (frontier, hashed) = (Frontier::new(), AtomicU64::new(0));
    let (fetched, sum) = std::thread::scope(|scope| {
        // One more thread of the request: it hashes the part while the connections fill it.
        let hasher = std::thread::Builder::new().name("serve-hash".into()).spawn_scoped(scope, || {
            let sum = hash_while_it_arrives(part, &frontier, &hashed);
            job::ring();
            sum
        });
        let fetched = transfer::download(url, out, connections, transfer::CHUNK, Some(&frontier), &begin, &quiet);
        job::end();
        frontier.close();
        let hasher = match hasher {
            Ok(hasher) => hasher,
            Err(e) => return (fetched, Err(Fail::failed(format!("the hash thread could not start: {e}")))),
        };
        if let Ok((size, _)) = &fetched {
            // what is left to hash when the last byte is there
            let progress = job::begin("check", "bytes", *size);
            while !hasher.is_finished() {
                progress.set(hashed.load(Ordering::Relaxed));
                job::wait_bell(Duration::from_millis(200), || hasher.is_finished());
            }
            progress.set(hashed.load(Ordering::Relaxed));
            job::end();
        }
        (fetched, hasher.join().unwrap_or_else(|_| Err(Fail::Failed("internal", "the hash thread failed".into()))))
    });
    let (size, how) = match (fetched, refusal.into_inner().unwrap()) {
        (_, Some(fail)) => return Err(fail),
        (Ok(fetched), None) => fetched,
        (Err(text), None) => return Err(net_fail(text)),
    };
    let (hashed, got) = sum?;
    if hashed != size {
        return Err(Fail::failed(format!("{part}: {hashed} of {size} bytes were hashed")));
    }
    if got != want {
        *wrong = true;
        return Err(Fail::refused("mismatch", format!("{url} did not arrive as expected: its SHA-256 is {got}, not {want}. Nothing was kept")));
    }
    job::step("place");
    check()?;
    std::fs::rename(part, out).map_err(|e| io(out, e))?;
    Ok(Done::new(format!("{out}: {}, SHA-256 as expected", size_text(size)))
        .f("out", out)
        .f("bytes", size)
        .f("sha256", got)
        .f("connections", how.connections)
        .f("ranges", how.ranges)
        .f("retries", how.retries)
        .f("resumed_from", how.resumed_from))
}

fn zip_ls(rq: &Rq) -> Result<Done, Fail> {
    let source = source_of(rq, "source")?;
    let pattern = rq.req.opt_str("match")?;
    net_keys(rq, false)?;
    rq.req.done()?;
    let pattern = pattern.map(|p| regex::Regex::new(&p).map_err(|e| Fail::usage(format!("zip-ls: \"match\": {e}")))).transpose()?;
    let zip = oneios_image::remotezip::RemoteZip::open(&source).map_err(net_fail)?;
    let mut count = 0;
    for info in zip.infos().iter().filter(|i| pattern.as_ref().is_none_or(|p| p.is_match(&i.filename))) {
        count += 1;
        let method = match info.compress_type {
            0 => "stored".to_string(),
            8 => "deflate".to_string(),
            other => other.to_string(),
        };
        rq.row("member", &[("name", json!(info.filename)), ("bytes", json!(info.file_size)), ("packed", json!(info.compress_size)), ("method", json!(method)), ("crc32", json!(format!("{:08x}", info.crc)))]);
    }
    Ok(Done::new(format!("{count} members")).f("members", count))
}

fn zip_get(rq: &Rq) -> Result<Done, Fail> {
    let source = source_of(rq, "source")?;
    let outdir = rq.write_path("outdir")?;
    let members = rq.req.objects("members", &["member", "name"])?;
    let resume = rq.req.flag("resume")?;
    let connections = net_keys(rq, true)?;
    rq.req.done()?;
    job::step("directory");
    let mut zip = oneios_image::remotezip::RemoteZip::open(&source).map_err(net_fail)?;
    zip.keep_parts = resume;
    // (member, the file's name in OUTDIR): a name is one plain file name
    let mut wanted = Vec::new();
    let (mut packed, mut plain) = (0u64, 0u64);
    for m in &members {
        let member = m[0].clone().expect("checked");
        let name = m[1].clone().unwrap_or_else(|| member.rsplit('/').next().unwrap_or(&member).to_string());
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Fail::usage(format!("zip-get: \"name\" of {member} must be one file name")));
        }
        let info = zip.find(&member).map_err(|text| Fail::refused("not-found", text))?;
        packed += info.compress_size;
        // (a deflated member is on the disk twice while it is inflated)
        plain += info.file_size + if info.compress_type == 0 { 0 } else { info.compress_size };
        wanted.push((member, name));
    }
    std::fs::create_dir_all(&outdir).map_err(|e| io(&outdir, e))?;
    rq.space(Path::new(&outdir), plain)?;
    job::begin("download", "bytes", packed).items(wanted.len() as u64);
    let mut total = 0;
    for (member, name) in &wanted {
        check()?;
        let out = format!("{}/{name}", outdir.trim_end_matches('/'));
        let (bytes, crc) = zip.get(member, &out, connections, oneios_image::remotezip::CHUNK, &quiet).map_err(net_fail)?;
        job::count_item();
        total += bytes;
        rq.row("member", &[("name", json!(member)), ("out", json!(out)), ("bytes", json!(bytes)), ("crc32", json!(format!("{crc:08x}")))]);
    }
    job::end();
    Ok(Done::new(format!("{} members, {}: each CRC-32 as the archive says", wanted.len(), size_text(total))).f("members", wanted.len()).f("bytes", total))
}

fn fetch(rq: &Rq) -> Result<Done, Fail> {
    let profile = rq.existing("profile")?;
    let indir = rq.write_path("indir")?;
    let only = rq.req.list("only")?;
    let source = match rq.req.opt_str("source")? {
        Some(_) => Some(source_of(rq, "source")?),
        None => None,
    };
    let resume = rq.req.flag("resume")?;
    let connections = net_keys(rq, true)?;
    rq.req.done()?;
    job::step("directory");
    let only = (!only.is_empty()).then(|| only.join(","));
    oneios_image::fetch(&profile, &indir, connections, only.as_deref(), source.as_deref(), resume).map_err(|e| {
        let text = e.to_string();
        // the profile's pins: a member that is not the pinned one
        if text.contains("the profile pins") || text.contains("the profile does not match") {
            Fail::refused("mismatch", text)
        } else if let Some(code) = oneios_image::transfer::take_failure() {
            // (the network gave up, or the server's file changed)
            Fail::Failed(code, text)
        } else {
            image_fail(e)
        }
    })?;
    Ok(Done::new(format!("the profile's members are in {indir}, checked against its pins")).f("indir", indir.as_str()).f("bytes", tree_bytes(Path::new(&indir))))
}

fn im4p(rq: &Rq) -> Result<Done, Fail> {
    let input = rq.existing("in")?;
    let out = rq.write_path("out")?;
    rq.req.done()?;
    job::step("decode");
    if let Some(parent) = Path::new(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
    }
    rq.space(Path::new(&out), std::fs::metadata(&input).map(|m| m.len() * 4).unwrap_or(0))?;
    let part = job::temp(format!("{out}.part"));
    let said = oneios_image::im4p::cmd(&input, &part.path().to_string_lossy()).map_err(image_fail)?;
    check()?;
    std::fs::rename(part.path(), &out).map_err(|e| io(&out, e))?;
    part.keep();
    // "KIND N bytes sha256 HEX -> PATH"
    let words: Vec<&str> = said.split_whitespace().collect();
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(Done::new(format!("{out}: {}", size_text(bytes))).f("out", out.as_str()).f("kind", words.first().copied().unwrap_or("")).f("bytes", bytes).f("sha256", words.get(4).copied().unwrap_or("")))
}

// ---------------------------------------------------------------- the import's receive check

/// How long a FIFO may stay without a writer before the copy gives up.
const WRITER_WAIT: Duration = Duration::from_secs(30);

/// Exactly `bytes` bytes of `from` (a file, or a FIFO the app writes) into `to.part`, hashed as they are written;
/// the size and the SHA-256 must be the ones given before the file gets its name and mode. This is
/// `oneios_root.sh receive`'s check, in one pass (the script hashes the file again after the copy).
fn receive(rq: &Rq) -> Result<Done, Fail> {
    use sha2::Digest;
    let from = rq.read_path("from")?;
    let to = rq.write_path("to")?;
    let bytes = rq.req.u64("bytes")?;
    let want = rq.req.str("sha256")?;
    let mode = rq.req.str("mode")?;
    let name = rq.req.opt_str("name")?;
    let lists = [("sums_list", rq.req.opt_str("sums_list")?), ("ids_list", rq.req.opt_str("ids_list")?)];
    let lists: Vec<Option<String>> = lists.into_iter().map(|(key, given)| given.map(|_| rq.write_path(key)).transpose()).collect::<Result<_, _>>()?;
    rq.req.done()?;
    if !is_sha256(&want) {
        return Err(Fail::usage("receive: \"sha256\" must be 64 lower-case hex digits"));
    }
    let mode = match mode.as_str() {
        "0644" => 0o644,
        "0755" => 0o755,
        _ => return Err(Fail::usage("receive: \"mode\" must be \"0644\" or \"0755\"")),
    };
    let name = name.unwrap_or_else(|| Path::new(&to).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    if name.is_empty() || name.contains('\n') {
        return Err(Fail::usage("receive: \"name\" must be the file's path in its list"));
    }
    let meta = std::fs::metadata(&from).map_err(|_| Fail::refused("not-found", format!("{from} is not there")))?;
    let fifo = std::os::unix::fs::FileTypeExt::is_fifo(&meta.file_type());
    if meta.is_file() && meta.len() != bytes {
        return Err(Fail::refused("mismatch", format!("{name}: {from} has {} bytes, {bytes} were announced", meta.len())));
    }
    if !meta.is_file() && !fifo {
        return Err(Fail::usage(format!("receive: {from} is not a file or a FIFO")));
    }
    fresh(&to)?;
    rq.space(Path::new(&to), bytes)?;
    let part = job::temp(format!("{to}.part"));
    remove(part.path());

    job::begin("copy", "bytes", bytes);
    // (non-blocking: a FIFO without a writer opens at once, and a stop is seen while no byte comes)
    let mut source = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&from).map_err(|e| io(&from, e))?;
    let mut file = std::fs::File::create(part.path()).map_err(|e| io(&part.path().display().to_string(), e))?;
    let mut hash = sha2::Sha256::new();
    let mut block = vec![0u8; 1 << 20];
    let mut got = 0u64;
    let began = Instant::now();
    while got < bytes {
        check()?;
        let mut poll = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&source), events: libc::POLLIN, revents: 0 };
        // Safety: poll on one descriptor of this process, in a local.
        let ready = unsafe { libc::poll(&mut poll, 1, 200) };
        if ready <= 0 {
            // (a FIFO that no writer opened in time: the copy ends, with no byte)
            if fifo && got == 0 && began.elapsed() >= WRITER_WAIT {
                break;
            }
            continue;
        }
        let want_now = ((bytes - got) as usize).min(block.len());
        match source.read(&mut block[..want_now]) {
            // the end: of a file, or of a FIFO whose writer closed. A FIFO no writer opened yet reads the same;
            // it is waited for a while
            Ok(0) if fifo && got == 0 && began.elapsed() < WRITER_WAIT => std::thread::sleep(Duration::from_millis(50)),
            Ok(0) => break,
            Ok(n) => {
                hash.update(&block[..n]);
                file.write_all(&block[..n]).map_err(|e| io(&part.path().display().to_string(), e))?;
                got += n as u64;
                job::count(n as u64);
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(io(&from, e)),
        }
    }
    file.flush().map_err(|e| io(&part.path().display().to_string(), e))?;
    drop(file);
    job::end();
    job::step("check");
    if got != bytes {
        return Err(Fail::refused("mismatch", format!("{name}: {got} of {bytes} bytes arrived")));
    }
    let sum = oneios_common::hex(&hash.finalize());
    if sum != want {
        return Err(Fail::refused("mismatch", format!("{name} did not arrive intact (SHA-256 differs)")));
    }
    job::step("place");
    check()?;
    std::fs::set_permissions(part.path(), std::fs::Permissions::from_mode(mode)).map_err(|e| io(&to, e))?;
    std::fs::rename(part.path(), &to).map_err(|e| io(&to, e))?;
    part.keep();
    // the lists, as `oneios_root.sh receive` writes them: "SHA256  NAME" and "DEV:INODE:SIZE:MTIME NAME"
    let placed = std::fs::metadata(&to).map_err(|e| io(&to, e))?;
    let lines = [format!("{sum}  {name}\n"), format!("{}:{}:{}:{} {name}\n", placed.dev(), placed.ino(), placed.size(), placed.mtime())];
    for (list, line) in lists.iter().zip(lines) {
        if let Some(list) = list {
            let mut f = std::fs::OpenOptions::new().append(true).create(true).open(list).map_err(|e| io(list, e))?;
            f.write_all(line.as_bytes()).map_err(|e| io(list, e))?;
        }
    }
    Ok(Done::new(format!("{name}: {}, SHA-256 as announced", size_text(bytes))).f("to", to.as_str()).f("bytes", bytes).f("sha256", sum))
}

// ---------------------------------------------------------------- the stage's hashes

/// The stage's SHA-256 check: every file a list names (`SHA256  PATH` lines, as the stage's `image.sha256` and
/// `runtime.sha256`), the files in the request's threads.
fn sums(rq: &Rq) -> Result<Done, Fail> {
    let dir = rq.existing("dir")?;
    let list = match rq.req.opt_str("list")? {
        Some(_) => rq.existing("list")?,
        None => format!("{}.sha256", dir.trim_end_matches('/')),
    };
    let strict = rq.req.flag("strict")?;
    rq.req.done()?;
    job::step("scan");
    let text = std::fs::read_to_string(&list).map_err(|e| Fail::refused("not-found", format!("{list}: {e}")))?;
    let mut wanted: Vec<(String, String)> = Vec::new();
    for (at, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let entry = line.split_once("  ").map(|(sum, path)| (sum.trim(), path.trim().trim_start_matches("./")));
        match entry {
            Some((sum, path)) if is_sha256(sum) && !path.is_empty() && !path.starts_with('/') && path.split('/').all(|p| !p.is_empty() && p != "..") => wanted.push((path.to_string(), sum.to_string())),
            _ => return Err(Fail::refused("mismatch", format!("{list}: line {} is not `SHA256  PATH` with a path below the folder", at + 1))),
        }
    }
    let full = |path: &str| format!("{}/{path}", dir.trim_end_matches('/'));
    let sizes: Vec<Option<u64>> = wanted.iter().map(|(path, _)| std::fs::symlink_metadata(full(path)).ok().filter(|m| m.is_file()).map(|m| m.len())).collect();
    let total: u64 = sizes.iter().flatten().sum();
    let mut unlisted = Vec::new();
    if strict {
        let names: HashSet<&str> = wanted.iter().map(|(path, _)| path.as_str()).collect();
        tree_files(Path::new(&dir), "", &mut unlisted);
        unlisted.retain(|path| !names.contains(path.as_str()));
        unlisted.sort();
    }
    job::begin("hash", "bytes", total).items(wanted.len() as u64);
    let oks: Vec<bool> = wanted
        .par_iter()
        .zip(sizes.par_iter())
        .map(|((path, want), size)| -> Result<bool, Fail> {
            let Some(size) = size else {
                job::row("file", &[("path", json!(path)), ("missing", json!(true)), ("ok", json!(false))]);
                job::count_item();
                return Ok(false);
            };
            let got = oneios_image::sha256_of(&full(path)).map_err(image_fail)?;
            let ok = got == *want;
            job::row("file", &[("path", json!(path)), ("bytes", json!(size)), ("sha256", json!(got)), ("ok", json!(ok))]);
            job::count_item();
            Ok(ok)
        })
        .collect::<Result<_, Fail>>()?;
    job::end();
    let bad: Vec<String> = wanted.iter().zip(&oks).zip(&sizes).filter(|((_, ok), _)| !**ok).map(|(((path, _), _), size)| format!("{path} ({})", if size.is_some() { "another SHA-256" } else { "missing" })).collect();
    if !bad.is_empty() || !unlisted.is_empty() {
        let some = |names: &[String]| format!("{}{}", names.iter().take(5).cloned().collect::<Vec<_>>().join(", "), if names.len() > 5 { ", ..." } else { "" });
        let mut parts = Vec::new();
        if !bad.is_empty() {
            parts.push(format!("{} of {} files are not as {list} lists them: {}", bad.len(), wanted.len(), some(&bad)));
        }
        if !unlisted.is_empty() {
            parts.push(format!("{} files of {dir} are not in the list: {}", unlisted.len(), some(&unlisted)));
        }
        return Err(Fail::refused("mismatch", parts.join("; ")));
    }
    Ok(Done::new(format!("{} files, {}: all as listed", wanted.len(), size_text(total))).f("files", wanted.len()).f("bytes", total))
}

fn hashes(rq: &Rq) -> Result<Done, Fail> {
    let image = rq.existing("image")?;
    let out = rq.write_path("out")?;
    let chunk = rq.req.opt_u64("chunk")?.unwrap_or(1 << 20);
    rq.req.done()?;
    if !(4096..=u32::MAX as u64).contains(&chunk) || !chunk.is_power_of_two() {
        return Err(Fail::usage("hashes: \"chunk\" must be a power of two from 4096 (the format's rule)"));
    }
    let size = std::fs::metadata(&image).map(|m| m.len()).unwrap_or(0);
    if let Some(parent) = Path::new(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
    }
    let part = job::temp(format!("{out}.part"));
    job::begin("hash", "bytes", size);
    let (bytes, sha256) = oneios_image::hashes_file(&image, &part.path().to_string_lossy(), chunk).map_err(image_fail)?;
    job::end();
    check()?;
    std::fs::rename(part.path(), &out).map_err(|e| io(&out, e))?;
    part.keep();
    Ok(Done::new(format!("{out}: {} chunks of {}", bytes.div_ceil(chunk), size_text(bytes))).f("out", out.as_str()).f("bytes", bytes).f("sha256", sha256).f("chunks", bytes.div_ceil(chunk)))
}

fn verify(rq: &Rq) -> Result<Done, Fail> {
    let set = rq.existing("set")?;
    let quick = rq.req.flag("quick")?;
    rq.req.done()?;
    if !Path::new(&set).is_dir() {
        return Err(Fail::usage("verify: \"set\" must be a set folder"));
    }
    job::begin("verify", "bytes", if quick { 0 } else { tree_bytes(Path::new(&set)) });
    let line = oneios_image::verify_set(&set, quick).map_err(|e| match e {
        // the tool's own finding: a file that is not the one manifest.json lists
        oneios_image::Fail::Error(text) => Fail::refused("mismatch", text),
        other => image_fail(other),
    })?;
    job::end();
    Ok(Done::new(line).f("set", set.as_str()))
}

// ---------------------------------------------------------------- the APFS reader

fn apfs_ls(rq: &Rq) -> Result<Done, Fail> {
    let image = rq.existing("image")?;
    let path = rq.req.opt_str("path")?.unwrap_or_default();
    let (volume, pick) = pick(rq)?;
    rq.req.done()?;
    let entries = oneios_image::apfs_tools::list(&image, &path, volume.as_deref(), pick).map_err(image_fail)?;
    for (mode, uid, gid, size, name) in &entries {
        rq.row("entry", &[("name", json!(name)), ("mode", json!(format!("{mode:06o}"))), ("uid", json!(uid)), ("gid", json!(gid)), ("bytes", json!(size))]);
    }
    Ok(Done::new(format!("{} entries", entries.len())).f("entries", entries.len()))
}

fn apfs_copy(rq: &Rq) -> Result<Done, Fail> {
    let image = rq.existing("image")?;
    let src = rq.req.str("src")?;
    let dst = rq.write_path("dst")?;
    let (volume, pick) = pick(rq)?;
    rq.req.done()?;
    fresh(&dst)?;
    let part = job::temp(format!("{dst}.part"));
    // (a part a killed process left)
    remove(part.path());
    let done = oneios_image::apfs_tools::copy_to(&image, &src, &part.path().to_string_lossy(), volume.as_deref(), pick).map_err(image_fail)?;
    job::step("place");
    check()?;
    std::fs::rename(part.path(), &dst).map_err(|e| io(&dst, e))?;
    part.keep();
    let was = if done.compressed == 0 { String::new() } else { format!(" ({} of them were kept compressed)", done.compressed) };
    let more = if done.skipped == 0 { String::new() } else { format!(", {} compressed files could not be decoded and were left out", done.skipped) };
    Ok(Done::new(format!("copied {} files, {}{was}{more}", done.files, size_text(done.bytes))).f("dst", dst.as_str()).f("files", done.files).f("bytes", done.bytes).f("compressed", done.compressed).f("skipped", done.skipped))
}

// ---------------------------------------------------------------- the root tweaks, the prelink, the package

fn tweaks(_: &Rq) -> Result<Done, Fail> {
    Err(Fail::refused(
        "not-ported",
        "The root tweaks are not in this tool yet: they are Python (userland/roottweak), which the phone does not have. Make the Data seed on a PC or a Mac until they are ported (tools/oneios-tool/todo_rust.md, section 7, group 4)",
    ))
}

fn prelink(rq: &Rq) -> Result<Done, Fail> {
    use oneios_prelink::build::{self, Refused};
    let args = build::Args {
        kernel: rq.existing("kernel")?,
        kernelcache: rq.existing("kernelcache")?,
        profile: rq.existing("profile")?,
        reviewed: rq.existing("reviewed")?,
        more_reviewed: rq.req.list("more_reviewed")?,
        pairs: rq.existing("pairs")?,
        stubs: rq.existing("stubs")?,
        xnuprelink: rq.existing("xnuprelink")?,
        reference: match rq.req.opt_str("reference")? {
            Some(_) => Some(rq.existing("reference")?),
            None => None,
        },
        out: rq.write_path("out")?,
        repo: None,
        // (`serve` starts no program: the record's two commits are the ones given, or null)
        no_git: true,
        oneios_commit: rq.req.opt_str("oneios_commit")?,
        xnuprelink_commit: rq.req.opt_str("xnuprelink_commit")?,
        remove_work: !rq.req.flag("keep_work")?,
    };
    rq.req.done()?;
    let size = |path: &str| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    // work holds the kernel twice, the reference, the rewritten kernelcache and the collection; OUT the collection
    let need = 2 * size(&args.kernel) + args.reference.as_deref().map_or(0, size) + 4 * size(&args.kernelcache) + (64 << 20);
    std::fs::create_dir_all(&args.out).map_err(|e| io(&args.out, e))?;
    rq.space(Path::new(&args.out), need)?;
    let built = build::build(&args).map_err(|e| match e.downcast_ref::<Refused>() {
        Some(Refused::NotFound(text)) => Fail::refused("not-found", text.clone()),
        Some(Refused::Mismatch(text)) => Fail::refused("mismatch", text.clone()),
        Some(Refused::Audit(text)) => Fail::refused("audit", text.clone()),
        None => Fail::failed(format!("{e:#}")),
    })?;
    Ok(Done::new(format!("{}: {}, audited", built.kc, size_text(built.bytes))).f("kc", built.kc.as_str()).f("bytes", built.bytes).f("sha256", built.sha256))
}

fn stubs_check(rq: &Rq) -> Result<Done, Fail> {
    let kc = rq.existing("kc")?;
    rq.req.done()?;
    job::step("check");
    match oneios_prelink::run(&["stubs-check".to_string(), kc.clone()]) {
        0 => Ok(Done::new(format!("{kc}: every stub branches to its .original symbol")).f("kc", kc.as_str())),
        _ => Err(Fail::refused("audit", format!("{kc}: a stub does not branch to its .original symbol, or the file could not be read (the lines before this say which)"))),
    }
}

fn package_check(rq: &Rq) -> Result<Done, Fail> {
    use oneios_package::pack;
    let dir = rq.existing("dir")?;
    rq.req.done()?;
    job::step("check");
    match pack::check(Path::new(&dir)) {
        Ok(_) => Ok(Done::new(format!("{dir}: no file of Apple's, and nothing the app's unpacker refuses")).f("dir", dir.as_str())),
        Err(pack::Stop::Refused(text)) => Err(Fail::refused("apple-file", text)),
        Err(pack::Stop::Failed(text)) => Err(Fail::failed(text)),
    }
}

// ---------------------------------------------------------------- the logs and the CPU check

fn ioslog_from(rq: &Rq) -> Result<Vec<String>, Fail> {
    // (our APFS reader only: the other one is a program)
    let mut words = vec![rq.existing("source")?, "--apfs".to_string(), "own".to_string()];
    if let Some(volume) = rq.req.opt_str("volume")? {
        words.extend(["--volume".to_string(), volume]);
    }
    Ok(words)
}

fn ioslog_boots(rq: &Rq) -> Result<Done, Fail> {
    let mut words = vec!["boots".to_string()];
    words.extend(ioslog_from(rq)?);
    rq.req.done()?;
    job::step("read");
    match oneios_ioslog::run(&words) {
        0 => Ok(Done::new("the boots the log holds")),
        _ => Err(Fail::failed("iOS's log could not be read (the lines before this say why)")),
    }
}

fn ioslog_show(rq: &Rq) -> Result<Done, Fail> {
    let mut words = vec!["show".to_string()];
    words.extend(ioslog_from(rq)?);
    let out = match rq.req.opt_str("out")? {
        Some(_) => Some(rq.write_path("out")?),
        None => None,
    };
    let format = rq.req.opt_str("format")?.unwrap_or_else(|| "text".into());
    if !["text", "csv", "jsonl"].contains(&format.as_str()) {
        return Err(Fail::usage("ioslog-show: \"format\" must be text, csv or jsonl"));
    }
    words.extend(["-f".to_string(), format]);
    for (key, option) in [("boot", "--boot"), ("since", "--since"), ("until", "--until"), ("subsystem", "--subsystem"), ("category", "--category"), ("type", "--type"), ("grep", "--grep")] {
        if let Some(value) = rq.req.opt_str(key)? {
            words.extend([option.to_string(), value]);
        }
    }
    for name in rq.req.list("proc")? {
        words.extend(["--proc".to_string(), name]);
    }
    if let Some(pid) = rq.req.opt_u64("pid")? {
        words.extend(["--pid".to_string(), pid.to_string()]);
    }
    if rq.req.flag("sort")? {
        words.push("--sort".to_string());
    }
    rq.req.done()?;
    let part = match &out {
        Some(out) => {
            if let Some(parent) = Path::new(out).parent() {
                std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
            }
            let part = job::temp(format!("{out}.part"));
            words.extend(["-o".to_string(), part.path().to_string_lossy().into_owned()]);
            Some(part)
        }
        None => None,
    };
    job::step("read");
    if oneios_ioslog::run(&words) != 0 {
        return Err(Fail::failed("iOS's log could not be read (the lines before this say why)"));
    }
    let entries = rq.row_field("shown", "entries").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut done = Done::new(format!("{entries} entries")).f("entries", entries);
    if let (Some(out), Some(part)) = (&out, part) {
        check()?;
        std::fs::rename(part.path(), out).map_err(|e| io(out, e))?;
        part.keep();
        done = done.f("out", out.as_str()).f("bytes", std::fs::metadata(out).map(|m| m.len()).unwrap_or(0));
    }
    Ok(done)
}

fn log_list(rq: &Rq) -> Result<Done, Fail> {
    rq.req.done()?;
    let there = oneios_phone::local::log_list(&rq.cx.stage).map_err(|e| Fail::failed(format!("{e:#}")))?;
    Ok(Done::new(format!("{there} logs are there")).f("logs", there))
}

fn log(rq: &Rq) -> Result<Done, Fail> {
    use oneios_phone::log::{rel_path_of, Filter, NAMED};
    let file = rq.req.opt_str("file")?.unwrap_or_else(|| "xnuhost".into());
    let tail = rq.req.opt_u64("tail")?;
    let grep = rq.req.list("grep")?;
    let filters = rq.req.list("filters")?;
    let (markers, since) = (rq.req.flag("markers")?, rq.req.opt_f64("since")?);
    let out = match rq.req.opt_str("out")? {
        Some(_) => Some(rq.write_path("out")?),
        None => None,
    };
    rq.req.done()?;
    rel_path_of(&file).map_err(|e| Fail::usage(format!("log: {e}")))?;
    if let Some(bad) = filters.iter().find(|f| !NAMED.iter().any(|(name, _)| name == f)) {
        return Err(Fail::usage(format!("log: no filter \"{bad}\" ({})", NAMED.map(|(name, _)| name).join(", "))));
    }
    let named: Vec<&str> = filters.iter().map(String::as_str).collect();
    let filter = Filter::new(&grep, &named, markers, since).map_err(|e| Fail::usage(format!("log: {e}")))?;
    let stage = rq.cx.stage.as_str();
    if !Path::new(&format!("{stage}/{}", rel_path_of(&file).expect("checked"))).is_file() {
        return Err(Fail::refused("not-found", format!("the log \"{file}\" is not there (log-list shows which are)")));
    }
    if let Some(parent) = out.as_ref().and_then(|out| Path::new(out).parent()) {
        std::fs::create_dir_all(parent).map_err(|e| io(&parent.display().to_string(), e))?;
    }
    let part = out.as_ref().map(|out| job::temp(format!("{out}.part")));
    let options = oneios_phone::local::LogOptions { file: &file, tail, out: part.as_ref().map(|p| p.path().to_str().unwrap_or("")) };
    let read = oneios_phone::local::log(stage, &options, filter).map_err(|e| Fail::failed(format!("{e:#}")))?;
    let mut done = Done::new(format!("{} of {} lines", read.matched, read.lines)).f("lines", read.lines).f("matched", read.matched).f("bytes", read.bytes);
    if let (Some(out), Some(part)) = (&out, part) {
        std::fs::rename(part.path(), out).map_err(|e| io(out, e))?;
        part.keep();
        done = done.f("out", out.as_str());
    }
    Ok(done)
}

fn cpu(rq: &Rq) -> Result<Done, Fail> {
    rq.req.done()?;
    let cpus = oneios_phone::local::cpu(&rq.cx.sys_root, &rq.cx.stage).map_err(|e| Fail::refused("not-found", format!("{e:#}")))?;
    Ok(Done::new(format!("{cpus} CPUs")).f("cpus", cpus))
}
