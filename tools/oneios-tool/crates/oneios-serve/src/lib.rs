//! `oneios-tool serve`: the one process the OneOS app starts as root for every iOS step on the phone
//! (tools/oneios-tool/README.md "serve" is the protocol; this is its code). One request per line on standard input,
//! JSON lines back on standard output; one request runs at a time and the others wait in a queue; a request is
//! stopped by a `cancel` request, by the end of the app's pipe, by SIGTERM or by the idle timeout; nothing is left
//! behind (no temporary file, no thread, no child process).
//!
//! The parts: `proto.rs` (the lines), `capture.rs` (what the steps print, as `line` events), `cmds.rs` (the
//! commands), and here the loop: the main thread reads the requests and the signals, one runner thread runs the
//! requests, and each request's work runs in a rayon pool of its own that ends with it.
//!
//! The phone's rules: the process is at nice 10 to 19 and never in a realtime class (`job::set_nice`, before any
//! thread starts); the heat, battery and session state come from named files only (`job::read_heat`, `session_open`);
//! no program is started; requests write only under the stage and the `--allow` folders.

mod capture;
mod cmds;
pub mod proto;

use capture::Capture;
use oneios_common::job::{self, Limits, Reading, Snapshot, Stop, Verdict};
use proto::{one_decimal, Done, Ev, Fail, Out, Req, PROTO};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Requests that may wait behind the running one.
pub const QUEUE_MAX: usize = 32;
/// Bytes that must stay free on a volume a request writes to.
pub const SPACE_RESERVE: u64 = 256 << 20;
/// How long the end of the process waits for a request that was told to stop.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// `line` events per request unless the request says otherwise.
const LINES_DEFAULT: u64 = 5000;

#[derive(clap::Args, Debug)]
pub struct Cli {
    /// The stage: requests write only under it (and each --allow), and its control/ and session/ files say whether
    /// an iOS session is open
    #[arg(long, value_name = "DIR", default_value = "/data/local/tmp/oneios")]
    stage: String,
    /// One more folder requests may write under (repeatable)
    #[arg(long, value_name = "DIR")]
    allow: Vec<String>,
    /// End when no request ran or waited and no line came for this many seconds (0: never)
    #[arg(long, value_name = "SECONDS", default_value_t = 300)]
    idle: u64,
    /// Worker threads of a request, 1 to 8 (default: the online CPUs, at most 8)
    #[arg(long, value_name = "N")]
    jobs: Option<usize>,
    /// The process's nice value, 10 to 19
    #[arg(long, value_name = "N", default_value_t = 10)]
    nice: i32,
    /// Degrees C: the case's pause and stop limit, the battery's pause and stop limit; or `off`
    #[arg(long, value_name = "A,B,C,D", default_value = "44,46,45,50")]
    thermal: String,
    /// Below this percent, and not charging, heavy requests are refused and a running one stops (0: no check)
    #[arg(long, value_name = "PERCENT", default_value_t = job::BATTERY_MIN_DEFAULT)]
    battery_min: u32,
    /// Run this request instead of reading standard input (repeatable, in order); the exit status is the last one's
    #[arg(long, value_name = "JSON")]
    request: Vec<String>,
    /// Read the phone's files (/sys, /proc, /dev/cpuset) below this folder: for tests
    #[arg(long, value_name = "DIR")]
    sys_root: Option<String>,
}

/// What every request sees of the process.
pub struct Cx {
    pub out: Arc<Out>,
    pub stage: String,
    /// The stage and the --allow folders, resolved.
    roots: Vec<PathBuf>,
    pub sys_root: String,
    pub jobs: usize,
    pub limits: Limits,
}

/// "--thermal A,B,C,D": four rising numbers, or off.
fn parse_thermal(text: &str) -> Result<Option<[f64; 4]>, String> {
    if text == "off" {
        return Ok(None);
    }
    let values: Vec<f64> = text.split(',').map(|v| v.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| format!("--thermal {text}: four numbers of degrees C, or off"))?;
    match values[..] {
        [a, b, c, d] if a < b && c < d && values.iter().all(|v| (0.0..=120.0).contains(v)) => Ok(Some([a, b, c, d])),
        _ => Err(format!("--thermal {text}: the case's pause and stop limit, then the battery's, each pause below its stop")),
    }
}

/// An iOS session is open: the stage's control/boot is this boot's id and control/state says so, or
/// session/xnuhost.pid names a live xnuhost. Named files only.
pub fn session_open(stage: &str, sys_root: &str) -> bool {
    let word = |path: String| std::fs::read_to_string(path).map(|t| t.trim().to_string()).unwrap_or_default();
    let boot = word(format!("{sys_root}/proc/sys/kernel/random/boot_id"));
    if !boot.is_empty() && word(format!("{stage}/control/boot")) == boot && matches!(word(format!("{stage}/control/state")).as_str(), "starting" | "running" | "returning") {
        return true;
    }
    let pid = word(format!("{stage}/session/xnuhost.pid"));
    !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) && std::fs::read(format!("{sys_root}/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("xnuhost"))
}

fn heat_fields(ev: Ev, r: &Reading) -> Ev {
    ev.opt("case", r.case.map(one_decimal)).opt("battery", r.battery.map(one_decimal)).opt("level", r.level).opt("charging", r.charging)
}

/// The free bytes of the volume that holds `path` (its nearest folder that exists).
fn free_bytes(path: &Path) -> Option<(PathBuf, u64)> {
    let mut at = path;
    while !at.exists() {
        at = at.parent()?;
    }
    let c = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    // Safety: statvfs fills the zeroed block it is given; the path is a C string.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((at.to_path_buf(), st.f_bavail as u64 * st.f_frsize as u64))
}

/// What a running request shows the app, and what its handler may ask about it.
pub struct ReqSink {
    id: String,
    out: Arc<Out>,
    steps: &'static [(&'static str, &'static str)],
    /// The last row of each kind (a handler reads a count a command gave as a row).
    rows: Mutex<HashMap<String, Vec<(String, Value)>>>,
    /// A refusal a hook inside another crate's code made (the space check of a copy).
    refusal: Mutex<Option<Fail>>,
    /// The numbers of the last `progress` line (step, done, total, items done, items).
    last_progress: Mutex<Option<(String, u64, u64, u64, u64)>>,
}

impl job::Sink for ReqSink {
    fn step(&self, step: &str) {
        let (n, text) = match self.steps.iter().position(|(name, _)| *name == step) {
            Some(at) => (at + 1, self.steps[at].1),
            None => (0, step),
        };
        self.out.send(&Ev::of("step", &self.id).f("step", step).f("n", n).f("of", self.steps.len()).f("text", text));
    }

    fn progress(&self, s: &Snapshot) {
        // (a step's last line is said again when the step ends: the same numbers are sent once)
        let now = (s.step.clone(), s.done, s.total, s.items_done, s.items_total);
        {
            let mut last = self.last_progress.lock().unwrap();
            if last.as_ref() == Some(&now) {
                return;
            }
            *last = Some(now);
        }
        let mut ev = Ev::of("progress", &self.id).f("step", s.step.as_str()).f("done", s.done).f("total", s.total).f("unit", s.unit);
        if s.total > 0 {
            ev = ev.f("percent", one_decimal((s.done.min(s.total) as f64) * 100.0 / s.total as f64));
        }
        ev = ev.opt("rate", s.rate.map(|r| r.round() as u64)).opt("eta", s.eta);
        if s.items_total > 0 {
            ev = ev.f("items_done", s.items_done).f("items_total", s.items_total);
        }
        self.out.send(&ev);
    }

    fn row(&self, kind: &str, fields: &[(&str, Value)]) {
        self.rows.lock().unwrap().insert(kind.to_string(), fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        self.out.send(&Ev::of("row", &self.id).f("kind", kind).fields(fields));
    }

    fn heat(&self, state: &str, why: &str, text: &str, reading: &Reading) {
        self.out.send(&heat_fields(Ev::of("heat", &self.id).f("state", state).f("why", why).f("text", text), reading));
    }

    fn event(&self, name: &str, fields: &[(&str, Value)]) {
        self.out.send(&Ev::of(name, &self.id).fields(fields));
    }
}

/// A request as its handler sees it.
pub struct Rq<'a> {
    pub cx: &'a Cx,
    pub req: &'a Req,
    /// The worker threads this request has.
    pub jobs: usize,
    sink: &'a ReqSink,
}

impl Rq<'_> {
    /// A path the request reads: absolute, without `..`.
    pub fn read_path(&self, key: &str) -> Result<String, Fail> {
        let path = self.req.str(key)?;
        plain_path(&path).map_err(|why| Fail::usage(format!("{}: \"{key}\" {why}", self.req.cmd)))?;
        Ok(path)
    }

    /// The same, and the file or folder must be there.
    pub fn existing(&self, key: &str) -> Result<String, Fail> {
        let path = self.read_path(key)?;
        if !Path::new(&path).exists() {
            return Err(Fail::refused("not-found", format!("{path} is not there")));
        }
        Ok(path)
    }

    /// A path the request writes: absolute, without `..`, not through a link, and under the stage or an --allow
    /// folder.
    pub fn write_path(&self, key: &str) -> Result<String, Fail> {
        let path = self.req.str(key)?;
        self.cx.may_write(&path).map_err(|why| Fail::refused("outside", why))?;
        Ok(path)
    }

    /// The `space` event, and the refusal when the volume that holds `path` has less than `need` and the reserve.
    pub fn space(&self, path: &Path, need: u64) -> Result<(), Fail> {
        cmds::space_for(self.sink, path, need)
    }

    /// A field of the last row of `kind` the command gave.
    pub fn row_field(&self, kind: &str, field: &str) -> Option<Value> {
        self.sink.rows.lock().unwrap().get(kind)?.iter().find(|(k, _)| k == field).map(|(_, v)| v.clone())
    }

    pub fn row(&self, kind: &str, fields: &[(&str, Value)]) {
        job::row(kind, fields);
    }
}

/// "1.2 GB", "340 MB", "12 KB", "7 bytes" (1,000-based, as storage is sold).
pub fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.0} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0} KB", b as f64 / 1e3),
        b => format!("{b} bytes"),
    }
}

/// Absolute, no `..`, no empty text.
fn plain_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(format!("must be an absolute path: {path}"));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("must not have \"..\": {path}"));
    }
    Ok(())
}

impl Cx {
    /// May a request write `path`: plain, under a root when its links are resolved, and not a link itself.
    pub fn may_write(&self, path: &str) -> Result<(), String> {
        plain_path(path)?;
        let p = Path::new(path);
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{path} is a link: this tool does not write through links"));
        }
        // the nearest folder that exists, resolved, then the rest of the path as it is
        let mut at = p;
        let mut rest = Vec::new();
        while !at.exists() {
            match (at.parent(), at.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_os_string());
                    at = parent;
                }
                _ => break,
            }
        }
        let mut real = std::fs::canonicalize(at).map_err(|e| format!("{}: {e}", at.display()))?;
        real.extend(rest.iter().rev());
        if self.roots.iter().any(|root| real.starts_with(root) && real != *root) {
            return Ok(());
        }
        Err(format!("{path} is outside the stage ({}): this tool writes only there", self.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")))
    }
}

// ---------------------------------------------------------------- the loop

static WAKE_FD: AtomicI32 = AtomicI32::new(-1);
static SIGNALLED: AtomicBool = AtomicBool::new(false);

/// Wake the main loop (from a signal handler too: write is async-signal-safe).
fn wake() {
    let fd = WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        // Safety: one byte to a pipe of this process; a full pipe (EAGAIN) means it is awake already.
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
    wake();
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Req>,
    running: Option<String>,
    /// The running request takes `pause` and `resume` (a network request that has not come to its last event).
    pausable: bool,
    /// Why the running request was told to stop (`cancelled`'s `by`).
    stop_by: Option<&'static str>,
    quit: bool,
    /// The ids whose last event was sent while they could still send one (the forced end).
    closed: HashSet<String>,
    requests: u64,
    cancelled: u64,
    /// The one-shot form's exit status: that of the last `--request` of the command line (`status_of`), whichever
    /// request ended last.
    status: i32,
    status_of: Option<String>,
}

struct Shared {
    cx: Cx,
    queue: Mutex<Queue>,
    work: Condvar,
    capture: Mutex<Option<Capture>>,
}

impl Shared {
    /// A request's last event, once: after it nothing with this id is sent.
    fn finish(&self, id: &str, ev: Ev) {
        let mut q = self.queue.lock().unwrap();
        if q.running.as_deref() == Some(id) && !q.closed.insert(id.to_string()) {
            return;
        }
        q.requests += 1;
        let status = match (ev.name(), ev.get("code").and_then(Value::as_str)) {
            ("ok", _) => 0,
            ("refused", Some("usage" | "unknown")) => 3,
            ("refused", _) => 2,
            ("failed", _) => 4,
            _ => {
                q.cancelled += 1;
                5
            }
        };
        if q.status_of.as_deref().is_none_or(|last| last == id) {
            q.status = status;
        }
        self.cx.out.send(&ev);
    }
}

fn cancelled_ev(req_id: &str, cmd: &str, by: &str, removed: usize, text: &str) -> Ev {
    Ev::of("cancelled", req_id).f("cmd", cmd).f("by", by).f("removed", removed).f("text", text)
}

/// One request, from `start` to its last event.
fn run_one(shared: &Shared, req: Req) {
    let cx = &shared.cx;
    let started = Instant::now();
    job::take_removed();
    let spec = cmds::find(&req.cmd).expect("only known commands are queued");
    let sink = ReqSink { id: req.id.clone(), out: cx.out.clone(), steps: spec.steps, rows: Default::default(), refusal: Default::default(), last_progress: Default::default() };
    let sink = Arc::new(sink);
    let own = (|| -> Result<(usize, u64), Fail> {
        let jobs = match req.opt_u64("jobs")? {
            Some(n) if (1..=job::JOBS_MAX as u64).contains(&n) => (n as usize).min(cx.jobs),
            Some(_) => return Err(Fail::usage(format!("{}: \"jobs\" must be 1 to {}", req.cmd, job::JOBS_MAX))),
            None => cx.jobs,
        };
        Ok((jobs, req.opt_u64("lines")?.unwrap_or(LINES_DEFAULT)))
    })();
    let (jobs, lines) = *own.as_ref().unwrap_or(&(cx.jobs, LINES_DEFAULT));
    let steps: Vec<Value> = spec.steps.iter().map(|(step, text)| json!({"step": step, "text": text})).collect();
    cx.out.send(&Ev::of("start", &req.id).f("cmd", req.cmd.as_str()).f("jobs", jobs).f("steps", steps));
    if let Some(capture) = shared.capture.lock().unwrap().as_ref() {
        capture.begin(&req.id, lines);
    }
    job::set_sink(Some(sink.clone()));
    *NOW.lock().unwrap() = Some(sink.clone());
    // (`wait` is heavy when it says so: the app's tests of the heat limits)
    let heavy = spec.heavy || (req.cmd == "wait" && req.flag("heavy").unwrap_or(false));
    let result = own.and_then(|_| {
        if heavy {
            if session_open(&cx.stage, &cx.sys_root) {
                return Err(Fail::refused("session", "An iOS session is open: return to One UI first"));
            }
            if let Verdict::Stop(stop) = job::verdict(&cx.limits, &job::read_heat(&cx.limits.root), None) {
                return Err(stop_fail(&stop));
            }
            job::set_limits(Some(cx.limits.clone()));
            // (at a pause limit the request waits here before its first step, as its workers do later)
            job::check().map_err(|stop| stop_fail(&stop))?;
        }
        // The request's own threads: made for it and joined before its last event.
        let handles = Mutex::new(Vec::new());
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .spawn_handler(|thread| {
                // (the main thread's stack size, which the commands have when they run alone)
                let handle = std::thread::Builder::new().name(format!("serve-job-{}", thread.index())).stack_size(8 << 20).spawn(move || thread.run())?;
                handles.lock().unwrap().push(handle);
                Ok(())
            })
            .build()
            .map_err(|e| Fail::failed(format!("the worker threads could not start: {e}")))?;
        let rq = Rq { cx, req: &req, jobs, sink: &sink };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.install(|| (spec.run)(&rq))));
        drop(pool);
        for handle in handles.into_inner().unwrap() {
            let _ = handle.join();
        }
        if spec.pausable {
            // A request that is paused does not end before it is resumed, also when its work is done (a pause
            // that came during its last step). Under the queue's lock a `pause` request finds it pausable or not:
            // no `paused` is said after this.
            loop {
                let held = job::hold();
                let mut q = shared.queue.lock().unwrap();
                // (a stop ends the hold: the request then ends as its work did)
                if held.is_err() || !job::paused() {
                    q.pausable = false;
                    break;
                }
            }
        }
        match result {
            Ok(result) => result,
            Err(panic) => {
                let text = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "a panic".to_string());
                Err(Fail::Failed("internal", format!("the tool itself failed in {}: {text}", req.cmd)))
            }
        }
    });
    job::end();
    let stop = job::stop_reason();
    job::set_limits(None);
    job::set_sink(None);
    *NOW.lock().unwrap() = None;
    let dropped = shared.capture.lock().unwrap().as_mut().map_or(0, Capture::end);
    let removed = job::take_removed();
    let by = shared.queue.lock().unwrap().stop_by.unwrap_or("request");
    let cmd = req.cmd.as_str();
    let result = match (result, sink.refusal.lock().unwrap().take()) {
        (Err(_), Some(refusal)) => Err(refusal),
        (result, _) => result,
    };
    let mut ev = match (result, stop) {
        (Ok(Done { text, fields }), _) => Ev::of("ok", &req.id).f("cmd", cmd).f("seconds", one_decimal(started.elapsed().as_secs_f64())).f("text", text).fields(&fields),
        (Err(_), Some(Stop::Cancelled)) => cancelled_ev(&req.id, cmd, by, removed, &match removed {
            0 => "stopped".to_string(),
            n => format!("stopped; {n} unfinished file{} removed", if n == 1 { " was" } else { "s were" }),
        }),
        (Err(_), Some(stop)) => fail_ev(&req.id, cmd, &stop_fail(&stop)),
        (Err(fail), None) => fail_ev(&req.id, cmd, &fail),
    };
    if dropped > 0 {
        ev = ev.f("lines_dropped", dropped);
    }
    shared.finish(&req.id, ev);
}

fn stop_fail(stop: &Stop) -> Fail {
    match stop {
        Stop::Cancelled => Fail::failed("stopped"),
        Stop::Heat(text) => Fail::refused("heat", text.clone()),
        Stop::Battery(text) => Fail::refused("battery", text.clone()),
    }
}

fn fail_ev(id: &str, cmd: &str, fail: &Fail) -> Ev {
    match fail {
        Fail::Refused(code, text) => Ev::of("refused", id).f("cmd", cmd).f("code", *code).f("text", text.as_str()),
        Fail::Failed(code, text) => Ev::of("failed", id).f("cmd", cmd).f("code", *code).f("text", text.as_str()),
    }
}

/// The running request's sink, for the hooks other crates call without knowing the request.
static NOW: Mutex<Option<Arc<ReqSink>>> = Mutex::new(None);

fn runner(shared: Arc<Shared>) {
    loop {
        let req = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if q.quit {
                    return;
                }
                if let Some(req) = q.waiting.pop_front() {
                    q.running = Some(req.id.clone());
                    q.pausable = cmds::find(&req.cmd).is_some_and(|spec| spec.pausable);
                    q.stop_by = None;
                    // (under the lock: a cancel of this request can only come after this)
                    job::clear_cancel();
                    break req;
                }
                q = shared.work.wait(q).unwrap();
            }
        };
        run_one(&shared, req);
        {
            let mut q = shared.queue.lock().unwrap();
            q.running = None;
            q.closed.clear();
        }
        wake();
    }
}

/// The threads this process has (`/proc/self/status`), and whether it has a child (`waitpid` says so without
/// waiting).
fn left_behind() -> (Option<u64>, u64) {
    let count = || -> Option<u64> { std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("Threads:")).and_then(|v| v.trim().parse().ok())) };
    // A thread that was joined has ended its work, but the kernel may still count it for a moment (the join is
    // woken while the thread exits): the count is read until it says one thread, for a quarter of a second at most.
    let mut threads = count();
    for _ in 0..50 {
        if threads.is_none_or(|n| n <= 1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        threads = count();
    }
    let mut status = 0;
    // Safety: waitpid with WNOHANG on this process's own children.
    let children = if unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) { 0 } else { 1 };
    (threads, children)
}

fn health(cx: &Cx, id: &str) -> Ev {
    let reading = job::read_heat(&cx.limits.root);
    let state = match job::verdict(&cx.limits, &reading, None) {
        _ if reading == Reading::default() => "unknown",
        Verdict::Go => "ok",
        Verdict::Pause(..) => "pause",
        Verdict::Stop(_) => "stop",
    };
    let text = match job::verdict(&cx.limits, &reading, None) {
        Verdict::Go => "ok".to_string(),
        Verdict::Pause(_, text) => text,
        Verdict::Stop(stop) => stop.to_string(),
    };
    heat_fields(Ev::of("ok", id).f("cmd", "health").f("seconds", 0.0).f("text", text), &reading).f("state", state).f("session", session_open(&cx.stage, &cx.sys_root))
}

/// One line of standard input.
fn take_line(shared: &Shared, line: &str) {
    let cx = &shared.cx;
    let error = |text: String| cx.out.send(&Ev::new("error").f("text", text));
    let req = match Req::parse(line) {
        Ok(req) => req,
        Err(text) => return error(text),
    };
    let mut q = shared.queue.lock().unwrap();
    let live = |q: &Queue, id: &str| q.running.as_deref() == Some(id) || q.waiting.iter().any(|r| r.id == id);
    match req.cmd.as_str() {
        "cancel" => {
            if q.running.as_deref() == Some(req.id.as_str()) {
                q.stop_by.get_or_insert("request");
                job::cancel();
            } else if let Some(at) = q.waiting.iter().position(|r| r.id == req.id) {
                let gone = q.waiting.remove(at).expect("a waiting request");
                drop(q);
                shared.finish(&gone.id, cancelled_ev(&gone.id, &gone.cmd, "request", 0, "was waiting; it did not start"));
            } else {
                drop(q);
                cx.out.send(&Ev::new("error").f("id", req.id.as_str()).f("code", "no-such-request").f("text", format!("cancel: no request {} is running or waiting", req.id)));
            }
        }
        // Hold the running request's transfer, or let it go on (README "Downloads on a mobile network"). The answer
        // is said under the queue's lock, so it comes before the request's last event, and under `job`'s hold, so
        // no byte is counted after `paused`.
        cmd @ ("pause" | "resume") => {
            // (the numbers of the step that counts; between two steps there are none)
            let said = |ev: &str, words: &str, now: Option<job::Snapshot>| {
                let ev = Ev::of(ev, &req.id);
                cx.out.send(&match now {
                    Some(s) => ev.f("step", s.step.as_str()).f("done", s.done).f("total", s.total).f("unit", s.unit).f("text", format!("{words} {} of {}", size_text(s.done), size_text(s.total))),
                    None => ev.f("text", words.trim_end_matches(" at").trim_end_matches(" from")),
                });
            };
            let refusal = match () {
                _ if q.running.as_deref() != Some(req.id.as_str()) && q.waiting.iter().any(|r| r.id == req.id) => Some(("not-running", "is waiting in the queue: there is nothing to pause yet")),
                _ if q.running.as_deref() != Some(req.id.as_str()) => Some(("no-such-request", "is not running or waiting")),
                _ if !q.pausable => Some(("not-pausable", "has no transfer to pause (download, zip-get and fetch have one, until their last step is done)")),
                _ if cmd == "pause" => {
                    job::pause(|now| said("paused", "paused at", now));
                    None
                }
                _ if job::resume(|now| said("resumed", "going on from", now)) => None,
                _ => Some(("not-paused", "is not paused")),
            };
            if let Some((code, why)) = refusal {
                drop(q);
                cx.out.send(&Ev::new("error").f("id", req.id.as_str()).f("code", code).f("text", format!("{cmd}: request {} {why}", req.id)));
            }
        }
        _ if live(&q, &req.id) => {
            drop(q);
            cx.out.send(&Ev::new("error").f("id", req.id.as_str()).f("code", "duplicate").f("text", format!("a request with the id {} is still running or waiting: this line was not taken", req.id)));
        }
        "ping" => {
            drop(q);
            match req.done() {
                Ok(()) => shared.finish(&req.id, Ev::of("ok", &req.id).f("cmd", "ping").f("seconds", 0.0).f("text", "here")),
                Err(fail) => shared.finish(&req.id, fail_ev(&req.id, "ping", &fail)),
            }
        }
        "health" => {
            drop(q);
            match req.done() {
                Ok(()) => shared.finish(&req.id, health(cx, &req.id)),
                Err(fail) => shared.finish(&req.id, fail_ev(&req.id, "health", &fail)),
            }
        }
        cmd if cmds::find(cmd).is_none() => {
            drop(q);
            shared.finish(&req.id, Ev::of("refused", &req.id).f("cmd", cmd).f("code", "unknown").f("text", format!("this tool has no command \"{cmd}\" (hello's cmds lists them)")));
        }
        // (32 behind the running one: a request the runner has not taken yet is the one that runs next)
        _ if q.waiting.len() >= QUEUE_MAX + usize::from(q.running.is_none()) => {
            drop(q);
            shared.finish(&req.id, Ev::of("refused", &req.id).f("cmd", req.cmd.as_str()).f("code", "busy").f("text", format!("{QUEUE_MAX} requests are waiting already")));
        }
        _ => {
            let ahead = q.waiting.len() + usize::from(q.running.is_some());
            if ahead > 0 {
                cx.out.send(&Ev::of("queued", &req.id).f("cmd", req.cmd.as_str()).f("ahead", ahead));
            }
            q.waiting.push_back(req);
            shared.work.notify_all();
        }
    }
}

/// Wait for standard input or a wake-up for at most `timeout`; what came.
fn wait_for(input: bool, wake_fd: i32, timeout: Option<Duration>) -> (bool, bool) {
    let mut fds = [libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 }];
    let count = if input { 2 } else { 1 };
    let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
    // Safety: poll on two descriptors of this process, in a local array.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), count as libc::nfds_t, ms) };
    if n <= 0 {
        return (false, false);
    }
    let woke = fds[0].revents != 0;
    if woke {
        let mut sink = [0u8; 64];
        // Safety: the pipe is non-blocking; this empties it.
        while unsafe { libc::read(wake_fd, sink.as_mut_ptr().cast(), sink.len()) } > 0 {}
    }
    (woke, input && fds[1].revents != 0)
}

/// `oneios-tool serve`; the exit status.
pub fn main(cli: Cli) -> i32 {
    // Before any thread starts: on Linux a nice value is a thread's, and a new thread takes its maker's.
    let niced = match job::set_nice(cli.nice) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("oneios-tool serve: --nice {}: {e}", cli.nice);
            return 3;
        }
    };
    let thermal = match parse_thermal(&cli.thermal) {
        Ok(thermal) => thermal,
        Err(text) => {
            eprintln!("oneios-tool serve: {text}");
            return 3;
        }
    };
    let jobs = match cli.jobs {
        None => job::default_jobs(),
        Some(n) if (1..=job::JOBS_MAX).contains(&n) => n,
        Some(n) => {
            eprintln!("oneios-tool serve: --jobs {n}: 1 to {}", job::JOBS_MAX);
            return 3;
        }
    };
    if cli.battery_min > 100 {
        eprintln!("oneios-tool serve: --battery-min {}: 0 to 100", cli.battery_min);
        return 3;
    }
    let mut requests = Vec::new();
    for text in &cli.request {
        match Req::parse(text) {
            Ok(req) if cmds::find(&req.cmd).is_some() || matches!(req.cmd.as_str(), "ping" | "health") => requests.push(text.clone()),
            Ok(req) => {
                eprintln!("oneios-tool serve: --request: no command \"{}\"", req.cmd);
                return 3;
            }
            Err(text) => {
                eprintln!("oneios-tool serve: --request: {text}");
                return 3;
            }
        }
    }
    job::set_jobs(jobs);
    let sys_root = cli.sys_root.clone().unwrap_or_default();
    // (a root that is not there yet counts as written: the first request may make the stage's folder)
    let roots: Vec<PathBuf> = std::iter::once(&cli.stage).chain(&cli.allow).map(|dir| std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir))).collect();
    let limits = Limits { root: sys_root.clone(), thermal, battery_min: cli.battery_min };

    // The wake-up pipe and the signals.
    let mut ends = [0 as libc::c_int; 2];
    // Safety: descriptor calls on a new pipe; the handlers only store a flag and write a byte.
    unsafe {
        if libc::pipe(ends.as_mut_ptr()) != 0 {
            eprintln!("oneios-tool serve: pipe: {}", std::io::Error::last_os_error());
            return 4;
        }
        for end in ends {
            libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(end, libc::F_SETFL, libc::O_NONBLOCK);
        }
        WAKE_FD.store(ends[1], Ordering::SeqCst);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
    let wake_fd = ends[0];
    let pipe = match capture::protocol_pipe() {
        Ok(pipe) => pipe,
        Err(e) => {
            eprintln!("oneios-tool serve: standard output: {e}");
            return 4;
        }
    };
    let out = Arc::new(Out::new(Box::new(pipe), Box::new(wake)));
    let capture = match Capture::start(out.clone()) {
        Ok(capture) => capture,
        Err(e) => {
            eprintln!("oneios-tool serve: {e}");
            return 4;
        }
    };
    let cx = Cx { out: out.clone(), stage: cli.stage.clone(), roots, sys_root, jobs, limits };
    let (nice, realtime) = job::nice_now();
    let mut cmds: Vec<&str> = vec!["ping", "health", "cancel", "pause", "resume"];
    cmds.extend(cmds::ALL.iter().map(|c| c.name));
    out.send(
        &Ev::new("hello")
            .f("proto", PROTO)
            .f("tool", env!("CARGO_PKG_VERSION"))
            .f("pid", std::process::id())
            .f("jobs", jobs)
            .f("nice", nice)
            .f("realtime", realtime)
            .f("idle", cli.idle)
            .f("stage", cli.stage.as_str())
            .f("thermal", thermal.map(|t| t.to_vec()))
            .f("battery_min", cli.battery_min)
            .f("cmds", cmds)
            .f("note", niced),
    );
    let shared = Arc::new(Shared { cx, queue: Mutex::new(Queue::default()), work: Condvar::new(), capture: Mutex::new(Some(capture)) });
    cmds::hooks();
    let run = {
        let shared = shared.clone();
        std::thread::Builder::new().name("serve-runner".into()).spawn(move || runner(shared)).expect("the runner thread")
    };
    let one_shot = !requests.is_empty();
    shared.queue.lock().unwrap().status_of = requests.last().and_then(|text| Req::parse(text).ok()).map(|req| req.id);
    for text in &requests {
        take_line(&shared, text);
    }

    let mut input: Vec<u8> = Vec::new();
    let mut last = Instant::now();
    let mut was_busy = false;
    let why = loop {
        let busy = {
            let q = shared.queue.lock().unwrap();
            q.running.is_some() || !q.waiting.is_empty()
        };
        if SIGNALLED.load(Ordering::SeqCst) {
            break "sigterm";
        }
        if out.broken() {
            break "stdout";
        }
        if one_shot && !busy {
            break "requests";
        }
        // (the idle time counts from the end of the last request, or the last line)
        if busy || was_busy {
            last = Instant::now();
        }
        was_busy = busy;
        let idle_left = (cli.idle > 0 && !busy && !one_shot).then(|| Duration::from_secs(cli.idle).saturating_sub(last.elapsed()));
        if idle_left.is_some_and(|left| left.is_zero()) {
            break "idle";
        }
        let (_, readable) = wait_for(!one_shot, wake_fd, idle_left);
        if !readable {
            continue;
        }
        let mut block = [0u8; 1 << 16];
        // Safety: a read of standard input into a local buffer.
        let n = unsafe { libc::read(0, block.as_mut_ptr().cast(), block.len()) };
        if n == 0 {
            break "eof";
        }
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break "eof";
        }
        last = Instant::now();
        input.extend_from_slice(&block[..n as usize]);
        while let Some(end) = input.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = input.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..end]);
            let text = text.trim();
            if !text.is_empty() {
                take_line(&shared, text);
            }
        }
        if input.len() > 1 << 20 {
            out.send(&Ev::new("error").f("text", "a line of more than 1 MiB is not a request: it was dropped"));
            input.clear();
        }
    };

    // The end: the waiting requests go, the running one is told to stop and waited for.
    let by = match why {
        "sigterm" => "sigterm",
        "idle" => "idle",
        _ => "eof",
    };
    let (dropped, running) = {
        let mut q = shared.queue.lock().unwrap();
        q.quit = true;
        if q.running.is_some() {
            q.stop_by.get_or_insert(by);
            job::cancel_at_the_end();
        }
        (q.waiting.drain(..).collect::<Vec<Req>>(), q.running.clone())
    };
    shared.work.notify_all();
    // how many requests this end cut short (the exit status says so)
    let cut = dropped.len() + usize::from(running.is_some());
    for req in dropped {
        shared.finish(&req.id, cancelled_ev(&req.id, &req.cmd, by, 0, "was waiting; it did not start"));
    }
    let until = Instant::now() + STOP_GRACE;
    let mut forced = false;
    while shared.queue.lock().unwrap().running.is_some() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            forced = true;
            break;
        }
        wait_for(false, wake_fd, Some(left.min(Duration::from_millis(200))));
    }
    if forced {
        // The request does not stop: its temporary files go now, its last event is said for it, and the exit
        // below ends its threads. (A download that was paused keeps its part, as when it stops in time; one with
        // "resume" has no temporary files.)
        let removed = if job::held_at_the_end() { 0 } else { job::remove_temps() };
        if let Some(id) = running {
            shared.finish(&id, cancelled_ev(&id, "", by, removed, "did not stop in time: the process ended it"));
        }
    } else {
        let _ = run.join();
        if let Some(capture) = shared.capture.lock().unwrap().take() {
            capture.restore();
        }
    }
    let (threads, children) = left_behind();
    let (count, cancelled, status) = {
        let q = shared.queue.lock().unwrap();
        (q.requests, q.cancelled, q.status)
    };
    let mut bye = Ev::new("bye").f("why", why).f("requests", count).f("cancelled", cancelled).opt("threads", threads).f("children", children);
    if forced {
        bye = bye.f("forced", true);
    }
    out.send(&bye);
    // (exit, not return: a request that did not stop still has threads, and they end here)
    std::process::exit(match () {
        _ if one_shot && why == "requests" => status,
        _ if cut > 0 => 5,
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thermal_option() {
        assert_eq!(parse_thermal("44,46,45,50"), Ok(Some(job::THERMAL_DEFAULT)));
        assert_eq!(parse_thermal(" 40.5, 42 ,43,47.5"), Ok(Some([40.5, 42.0, 43.0, 47.5])));
        assert_eq!(parse_thermal("off"), Ok(None));
        for bad in ["", "44", "44,46,45", "46,44,45,50", "44,46,50,45", "a,b,c,d", "44,46,45,500"] {
            assert!(parse_thermal(bad).is_err(), "{bad}");
        }
    }

    fn cx(stage: &Path, allow: &[&Path]) -> Cx {
        let out = Arc::new(Out::new(Box::new(std::io::sink()), Box::new(|| {})));
        let roots = std::iter::once(stage).chain(allow.iter().copied()).map(|d| std::fs::canonicalize(d).unwrap()).collect();
        Cx { out, stage: stage.to_string_lossy().into_owned(), roots, sys_root: String::new(), jobs: 2, limits: Limits { root: String::new(), thermal: None, battery_min: 0 } }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oneios-serve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_stay_under_the_stage() {
        let top = tmp("roots");
        let (stage, allowed, other) = (top.join("stage"), top.join("allowed"), top.join("other"));
        for d in [&stage, &allowed, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(&other, stage.join("escape")).unwrap();
        std::os::unix::fs::symlink(stage.join("inside"), stage.join("link-in")).unwrap();
        std::fs::create_dir_all(stage.join("inside")).unwrap();
        let cx = cx(&stage, &[&allowed]);
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        assert_eq!(cx.may_write(&s(stage.join("work/a/b.img"))), Ok(()), "folders that are not there yet");
        assert_eq!(cx.may_write(&s(allowed.join("x"))), Ok(()));
        assert_eq!(cx.may_write(&s(stage.join("link-in/x"))), Ok(()), "a link that stays inside");
        for (bad, why) in [
            (s(other.join("x")), "outside the stage"),
            (s(stage.join("escape/x")), "outside the stage"),
            (s(stage.join("escape")), "is a link"),
            (s(stage.clone()), "outside the stage"),
            (format!("{}/../other/x", stage.display()), "must not have \"..\""),
            ("relative/x".to_string(), "must be an absolute path"),
            ("/etc/passwd".to_string(), "outside the stage"),
        ] {
            let e = cx.may_write(&bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn a_session_is_open_by_the_stages_files() {
        let (stage, root) = (tmp("session-stage"), tmp("session-root"));
        let put = |base: &Path, rel: &str, text: &str| {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let open = || session_open(stage.to_str().unwrap(), root.to_str().unwrap());
        assert!(!open(), "no stage files");
        put(&root, "proc/sys/kernel/random/boot_id", "boot-1\n");
        put(&stage, "control/boot", "boot-1\n");
        put(&stage, "control/state", "running\n");
        assert!(open());
        put(&stage, "control/state", "returned\n");
        assert!(!open());
        put(&stage, "control/state", "starting\n");
        put(&stage, "control/boot", "boot-0\n");
        assert!(!open(), "a state left by an earlier boot");
        put(&stage, "session/xnuhost.pid", "77\n");
        assert!(!open(), "the pid is of no process");
        put(&root, "proc/77/cmdline", "/data/local/tmp/oneios/runtime/bin/xnuhost\0");
        assert!(open(), "xnuhost is alive");
        put(&root, "proc/77/cmdline", "sh\0");
        assert!(!open(), "the pid is another program's now");
        for d in [stage, root] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn sizes_in_plain_words() {
        assert_eq!([size_text(7), size_text(12_345), size_text(340_000_000), size_text(10_359_930_880)], ["7 bytes", "12 KB", "340 MB", "10.4 GB"]);
    }
}
