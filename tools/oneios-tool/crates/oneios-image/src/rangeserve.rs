//! `oneios-tool rangeserve DIR PORT` (was scripts/serve/rangeserve.py, retired 2026-11-13): serve the files of one
//! folder over HTTP with byte ranges, so a download manager can fetch one file in many parallel parts (and from
//! several mirrors of the same server at once). scripts/serve/serve-image.sh starts it.
//!
//!   rangeserve DIR PORT [OPTION...]   binds 127.0.0.1 only: reach it through a tunnel, never from the LAN
//!
//! Only plain files directly in DIR (or symlinks to files) are served; no listing beyond them, no paths with '/'.
//! Each answer names the file's `Last-Modified` and an `ETag` of its time and size (nginx's form), so that a
//! download that is gone on with later can tell that the file is still the same one (transfer.rs).
//!
//! The options make it a bad network, for the tests and the measurements of a download's retries
//! (crates/oneios-tool/tests/serve.rs, transfer_tests.rs; README "Downloads on a mobile network"):
//!   --drop-every BYTES   a GET's body ends, and its connection closes, after a random count of bytes between
//!                        half of BYTES and BYTES + half (a body that is shorter than that arrives whole)
//!   --fail-first N       the first N requests are answered 503
//!   --fail-every N       every Nth request after those is answered 503
//!   --no-range           a Range header is ignored: 200 and the whole file
//!   --rate BYTES         each connection gives at most BYTES a second (a slow line: a download that can be
//!                        paused in the middle)
//!   --etag TEXT          this ETag instead of the file's own (`none`: no ETag and no Last-Modified)
//!
//! The server is 200 lines over std's TcpListener, a thread per connection (Python's http.server did the same
//! job): GET and HEAD, one range per request, HTTP/1.1 with connections kept open. No HTTP library: what is
//! served is one fixed set of headers, and the rule of which requests are answered is the point of the file.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const CHUNK: usize = 1 << 20;

/// What the options ask for (none: a server that answers every request whole).
#[derive(Clone, Debug, Default)]
pub struct Faults {
    pub drop_every: u64,
    pub fail_first: u64,
    pub fail_every: u64,
    pub no_range: bool,
    pub rate: u64,
    pub etag: Option<String>,
}

/// One server: its folder, its faults, and how many requests it has answered.
struct Server {
    root: PathBuf,
    faults: Faults,
    requests: AtomicU64,
}

/// 0 to `below` (std's hasher keys are random per `RandomState`).
fn random(below: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish() % below.max(1)
}

/// Seconds since 1970 as an HTTP date (`Sat, 14 Nov 2026 21:41:21 GMT`). The civil date from the day count is the
/// usual era arithmetic (days since 0000-03-01 in 400-year eras).
fn http_date(seconds: u64) -> String {
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let z = days + 719_468;
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + u64::from(month <= 2);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
    let name = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(month - 1) as usize];
    format!("{weekday}, {day:02} {name} {year} {:02}:{:02}:{:02} GMT", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// `urllib.parse.unquote`: %XX to its byte; the result as UTF-8 (a broken sequence: the replacement character).
fn unquote(text: &str) -> String {
    let (bytes, mut out, mut at) = (text.as_bytes(), Vec::new(), 0);
    while at < bytes.len() {
        let hex = bytes.get(at + 1..at + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file a request's path names: a plain file directly in the folder, not hidden.
fn file_of(root: &Path, target: &str) -> Option<PathBuf> {
    // (the path alone: no query, no fragment)
    let path = target.split(['?', '#']).next().unwrap_or("");
    let name = unquote(path);
    let name = name.trim_start_matches('/');
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    Some(root.join(name)).filter(|path| path.is_file())
}

/// What a Range header asks of a file of `size` bytes.
#[derive(Debug, PartialEq)]
enum Range {
    /// No header, or one that is not a single `bytes=` range: the whole file.
    Whole,
    /// First and last byte.
    Part(u64, u64),
    /// A range that the file does not have, or that is no number: 416.
    Bad,
}

fn range_of(header: Option<&str>, size: u64) -> Range {
    let Some(spec) = header.and_then(|h| h.strip_prefix("bytes=")).filter(|h| !h.contains(',')) else { return Range::Whole };
    let (a, b) = spec.trim().split_once('-').unwrap_or((spec.trim(), ""));
    let number = |text: &str| text.trim().parse::<i128>().ok();
    if a.is_empty() {
        // the last N bytes
        return match number(b) {
            // (of an empty file there is no last byte: the script answered 206 with a range of no bytes)
            Some(n) if n > 0 && size > 0 => Range::Part((size as i128 - n).max(0) as u64, size - 1),
            _ => Range::Bad,
        };
    }
    let (Some(start), Some(end)) = (number(a), if b.is_empty() { Some(size as i128 - 1) } else { number(b) }) else { return Range::Bad };
    if start < 0 || start >= size as i128 || end < start {
        return Range::Bad;
    }
    Range::Part(start as u64, (end as u64).min(size - 1))
}

impl Server {
    /// One request's answer: the status line and headers, then the body for a GET. Returns the status, and whether
    /// the connection ends with it.
    fn answer(&self, method: &str, target: &str, range: Option<&str>, out: &mut TcpStream) -> std::io::Result<(u16, bool)> {
        let faults = &self.faults;
        let count = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= faults.fail_first || (faults.fail_every > 0 && (count - faults.fail_first).is_multiple_of(faults.fail_every)) {
            write!(out, "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok((503, true));
        }
        let Some(path) = file_of(&self.root, target) else {
            let body = "404 Not Found\n";
            write!(out, "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
            if method == "GET" {
                out.write_all(body.as_bytes())?;
            }
            return Ok((404, true));
        };
        let meta = std::fs::metadata(&path)?;
        let size = meta.len();
        let (status, start, end) = match range_of(range.filter(|_| !faults.no_range), size) {
            Range::Bad => {
                write!(out, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n")?;
                return Ok((416, false));
            }
            Range::Whole => (200, 0, size),
            Range::Part(start, end) => (206, start, end + 1),
        };
        write!(out, "HTTP/1.1 {status} {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n", if status == 206 { "Partial Content" } else { "OK" }, end - start)?;
        if !faults.no_range {
            out.write_all(b"Accept-Ranges: bytes\r\n")?;
        }
        // which file this is, for a download that is gone on with later
        let changed = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |t| t.as_secs());
        match faults.etag.as_deref() {
            Some("none") => {}
            Some(etag) => write!(out, "ETag: \"{etag}\"\r\nLast-Modified: {}\r\n", http_date(changed))?,
            None => write!(out, "ETag: \"{changed:x}-{size:x}\"\r\nLast-Modified: {}\r\n", http_date(changed))?,
        }
        if status == 206 {
            write!(out, "Content-Range: bytes {start}-{}/{size}\r\n", end - 1)?;
        }
        out.write_all(b"\r\n")?;
        let mut ends = false;
        if method == "GET" {
            let mut left = (end - start) as usize;
            // (the fault: this body breaks off after about `drop_every` bytes, when it is longer than that)
            if faults.drop_every > 0 {
                let cut = (faults.drop_every / 2 + random(faults.drop_every)).max(1) as usize;
                (left, ends) = if cut < left { (cut, true) } else { (left, false) };
            }
            let mut file = File::open(&path)?;
            file.seek(SeekFrom::Start(start))?;
            // (a slow line: a 20th of a second's bytes, then a 20th of a second's sleep)
            let most = if faults.rate > 0 { (faults.rate / 20).clamp(1, CHUNK as u64) as usize } else { CHUNK };
            let mut block = vec![0u8; most.min(left)];
            while left > 0 {
                let n = file.read(&mut block[..left.min(most)])?;
                if n == 0 {
                    break;
                }
                out.write_all(&block[..n])?;
                left -= n;
                if faults.rate > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        Ok((status, ends))
    }

    /// The requests of one connection, until it closes or asks to.
    fn serve(&self, stream: TcpStream) -> std::io::Result<()> {
        let mut out = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let request = line.trim_end().to_string();
            let (mut range, mut close) = (None, false);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header)? == 0 || header.trim_end().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    match name.to_ascii_lowercase().as_str() {
                        "range" => range = Some(value.trim().to_string()),
                        "connection" => close = value.trim().eq_ignore_ascii_case("close"),
                        _ => {}
                    }
                }
            }
            let words: Vec<&str> = request.split_whitespace().collect();
            let (status, ends) = match words[..] {
                [method @ ("GET" | "HEAD"), target, ..] => self.answer(method, target, range.as_deref(), &mut out)?,
                _ => {
                    write!(out, "HTTP/1.1 501 Unsupported method\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    (501, true)
                }
            };
            // the log: one line per request on stderr, as http.server wrote one
            eprintln!("\"{request}\" {status} -{}", if ends && status < 400 { " (dropped)" } else { "" });
            if close || ends {
                return Ok(());
            }
        }
    }
}

/// Serve `root` on `listener` until the process ends, a thread per connection.
pub fn serve_on(listener: TcpListener, root: PathBuf, faults: Faults) {
    let server = Arc::new(Server { root, faults, requests: AtomicU64::new(0) });
    for stream in listener.incoming().flatten() {
        let server = server.clone();
        // (a client that goes away in the middle of a file ends its own thread: nothing to report)
        std::thread::spawn(move || server.serve(stream));
    }
}

const USAGE: &str = "usage: oneios-tool rangeserve DIR PORT [--drop-every BYTES] [--fail-first N] [--fail-every N] [--no-range] [--rate BYTES] [--etag TEXT]\n(binds 127.0.0.1 only; the options make it a bad network, for tests)";

/// The options after DIR and PORT.
fn faults_of(words: &[String]) -> Option<Faults> {
    let (mut faults, mut words) = (Faults::default(), words.iter());
    while let Some(word) = words.next() {
        let mut number = || words.next().and_then(|n| n.parse::<u64>().ok());
        match word.as_str() {
            "--drop-every" => faults.drop_every = number()?,
            "--fail-first" => faults.fail_first = number()?,
            "--fail-every" => faults.fail_every = number()?,
            "--no-range" => faults.no_range = true,
            "--rate" => faults.rate = number()?,
            "--etag" => faults.etag = Some(words.next()?.clone()),
            _ => return None,
        }
    }
    Some(faults)
}

/// `oneios-tool rangeserve DIR PORT [OPTION...]`; serves until it is ended.
pub fn main(args: &[String]) -> i32 {
    let (Some(dir), Some(port), Some(faults)) = (args.first(), args.get(1), faults_of(args.get(2..).unwrap_or_default())) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let (Ok(root), Ok(port)) = (std::fs::canonicalize(dir), port.parse::<u16>()) else {
        eprintln!("rangeserve: {dir} is no folder, or {port} is no port\n{USAGE}");
        return 2;
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("rangeserve: 127.0.0.1:{port}: {e}");
            return 1;
        }
    };
    serve_on(listener, root, faults);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_as_the_script_read_them() {
        assert_eq!(range_of(None, 100), Range::Whole);
        assert_eq!(range_of(Some("items=0-1"), 100), Range::Whole);
        assert_eq!(range_of(Some("bytes=0-1,5-6"), 100), Range::Whole);
        assert_eq!(range_of(Some("bytes=0-9"), 100), Range::Part(0, 9));
        assert_eq!(range_of(Some("bytes=90-"), 100), Range::Part(90, 99));
        assert_eq!(range_of(Some("bytes=90-500"), 100), Range::Part(90, 99));
        assert_eq!(range_of(Some("bytes=-10"), 100), Range::Part(90, 99));
        assert_eq!(range_of(Some("bytes=-500"), 100), Range::Part(0, 99));
        assert_eq!(range_of(Some("bytes=5"), 100), Range::Part(5, 99));
        assert_eq!(range_of(Some("bytes=-5"), 0), Range::Bad);
        for bad in ["bytes=100-", "bytes=9-3", "bytes=-0", "bytes=-", "bytes=a-b", "bytes=0-9"] {
            assert_eq!(range_of(Some(bad), if bad == "bytes=0-9" { 0 } else { 100 }), Range::Bad, "{bad}");
        }
    }

    #[test]
    fn only_plain_files_directly_in_the_folder() {
        let root = std::env::temp_dir().join(format!("oneios-rangeserve-test.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        for name in ["a b.bin", ".hidden", "sub/inner"] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        assert_eq!(file_of(&root, "/a%20b.bin?x=1"), Some(root.join("a b.bin")));
        for target in ["/", "/.hidden", "/sub", "/sub/inner", "/sub%2Finner", "/missing", "/..", "/%2e%2e"] {
            assert_eq!(file_of(&root, target), None, "{target}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
