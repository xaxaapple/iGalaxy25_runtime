//! What the steps print, as `line` events. The tools' commands print their own lines to standard output and
//! standard error; in `serve` standard output is the app's pipe. So `serve` keeps the app's pipe for itself and puts
//! a pipe of its own in place of the process's standard output and one in place of its standard error. A thread
//! reads each and sends every line as `{"ev":"line","id":...,"stream":"out"|"err","text":...}` of the request that
//! runs. Nothing a step prints can reach the app as anything but a `line`.
//!
//! Before a request's last event the runner waits until both threads have seen a mark it wrote after the step's
//! last line (`sync`), so the lines come before the last event.

use crate::proto::{Ev, Out, LINE_MAX};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A mark is this, a number, and a line end. No tool prints these control characters.
const MARK: &str = "\u{1e}\u{1e}oneios-serve-mark ";

/// The request whose lines are sent, and how many more may be.
struct Current {
    id: String,
    left: u64,
    dropped: u64,
}

#[derive(Default)]
struct State {
    current: Mutex<Option<Current>>,
    /// The newest mark each stream's thread has seen.
    seen: Mutex<[u64; 2]>,
    came: Condvar,
}

pub struct Capture {
    state: Arc<State>,
    /// The process's own standard output and error, put back by `restore`.
    saved: [RawFd; 2],
    threads: Vec<std::thread::JoinHandle<()>>,
    marks: u64,
}

fn sys(result: libc::c_int) -> std::io::Result<libc::c_int> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

/// A copy of standard output for the protocol, to be taken before `Capture::start`.
pub fn protocol_pipe() -> std::io::Result<File> {
    // Safety: dup of a descriptor this process has; the new one is owned by the File.
    let fd = sys(unsafe { libc::fcntl(1, libc::F_DUPFD_CLOEXEC, 3) })?;
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Capture {
    /// From now on what is written to descriptors 1 and 2 is read here.
    pub fn start(out: Arc<Out>) -> std::io::Result<Capture> {
        let state = Arc::new(State::default());
        let mut saved = [-1; 2];
        let mut threads = Vec::new();
        for (index, target) in [1, 2].into_iter().enumerate() {
            let mut ends = [0 as libc::c_int; 2];
            // Safety: plain descriptor calls; each new descriptor is owned below or closed.
            unsafe {
                sys(libc::pipe(ends.as_mut_ptr()))?;
                for end in ends {
                    sys(libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC))?;
                }
                saved[index] = sys(libc::fcntl(target, libc::F_DUPFD_CLOEXEC, 3))?;
                sys(libc::dup2(ends[1], target))?;
                libc::close(ends[1]);
            }
            let read = unsafe { OwnedFd::from_raw_fd(ends[0]) };
            let (state, out) = (state.clone(), out.clone());
            let stream = if index == 0 { "out" } else { "err" };
            threads.push(std::thread::Builder::new().name(format!("serve-{stream}")).spawn(move || reader(File::from(read), index, stream, &state, &out))?);
        }
        Ok(Capture { state, saved, threads, marks: 0 })
    }

    /// The lines that come from now on are request `id`'s; at most `lines` of them are sent.
    pub fn begin(&self, id: &str, lines: u64) {
        *self.state.current.lock().unwrap() = Some(Current { id: id.to_string(), left: lines, dropped: 0 });
    }

    /// Wait until everything the request printed was sent; then its lines end. Returns how many were not sent.
    pub fn end(&mut self) -> u64 {
        self.sync();
        self.state.current.lock().unwrap().take().map_or(0, |c| c.dropped)
    }

    fn sync(&mut self) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        self.marks += 1;
        let mark = format!("{MARK}{}\n", self.marks);
        for target in [1, 2] {
            // Safety: a write of a local buffer to a descriptor this process has.
            unsafe { libc::write(target, mark.as_ptr().cast(), mark.len()) };
        }
        let until = Instant::now() + Duration::from_secs(5);
        let mut seen = self.state.seen.lock().unwrap();
        while seen.iter().any(|s| *s < self.marks) {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            seen = self.state.came.wait_timeout(seen, left).unwrap().0;
        }
    }

    /// Put the process's own standard output and error back and end the two threads.
    pub fn restore(mut self) {
        for (index, target) in [1, 2].into_iter().enumerate() {
            // Safety: the saved descriptors are this object's; dup2 closes the pipe's last write end.
            unsafe {
                libc::dup2(self.saved[index], target);
                libc::close(self.saved[index]);
            }
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn reader(mut pipe: File, index: usize, stream: &str, state: &State, out: &Out) {
    let _ = pipe.as_raw_fd();
    let mut pending: Vec<u8> = Vec::new();
    let mut block = vec![0u8; 1 << 16];
    loop {
        let n = match pipe.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        pending.extend_from_slice(&block[..n]);
        let mut from = 0;
        while let Some(end) = pending[from..].iter().position(|b| *b == b'\n') {
            let raw = String::from_utf8_lossy(&pending[from..from + end]).into_owned();
            from += end + 1;
            // a mark after text that had no line end: the text is a line of its own
            let (text, mark) = match raw.find(MARK) {
                Some(at) => (&raw[..at], raw[at + MARK.len()..].trim().parse::<u64>().ok()),
                None => (raw.as_str(), None),
            };
            if !text.is_empty() || mark.is_none() {
                line(text.trim_end_matches('\r'), stream, state, out);
            }
            if let Some(mark) = mark {
                state.seen.lock().unwrap()[index] = mark;
                state.came.notify_all();
            }
        }
        pending.drain(..from);
        // a line without an end that grows past the limit is sent in pieces
        if pending.len() > 4 * LINE_MAX {
            let text = String::from_utf8_lossy(&pending).into_owned();
            pending.clear();
            line(&text, stream, state, out);
        }
    }
}

fn line(text: &str, stream: &str, state: &State, out: &Out) {
    let mut current = state.current.lock().unwrap();
    let Some(current) = current.as_mut() else { return };
    if current.left == 0 {
        current.dropped += 1;
        return;
    }
    current.left -= 1;
    let ev = Ev::of("line", &current.id).f("stream", stream);
    let ev = match text.char_indices().nth(LINE_MAX) {
        Some((at, _)) => ev.f("text", &text[..at]).f("cut", true),
        None => ev.f("text", text),
    };
    out.send(&ev);
}
