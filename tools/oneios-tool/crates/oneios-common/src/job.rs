//! job: what long work of any tool shares when it runs for the app (`oneios-tool serve`, README "serve"): the stop
//! flag, the thread count, the nice value, progress with a rate and an ETA, the temporary files that a stop removes,
//! and the heat and battery limits.
//!
//! One request runs at a time in `serve`, so the state is the process's. A command that runs alone (the one-shot
//! forms) sees none of it: nothing is cancelled, no one listens to progress, no limit is set, and every call here
//! costs one atomic load.
//!
//! The phone's rules this keeps: never a realtime class (`set_nice` also leaves one the process was started with),
//! and the heat and battery state is read from **named files only**, the thermal bridge's own
//! (`runtime/bridges/thermal/thermal.c`: `battery/lrp`, `sec-ap-thermistor/temperature`, `battery/temp`), never from a
//! folder list or a walk of /sys or /proc (a root tree walk panicked the phone's kernel on 2026-11-12).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- the stop

/// Why work ends before it is done.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    /// A cancel request, the end of the app's pipe, SIGTERM.
    Cancelled,
    /// The case or the battery is at its stop limit; the text is the reason in plain words.
    Heat(String),
    /// The battery is below the limit and not charging.
    Battery(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Cancelled => f.write_str("stopped"),
            Stop::Heat(text) | Stop::Battery(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Stop {}

static CANCEL: AtomicBool = AtomicBool::new(false);
/// The cancel is the end of the process (the app's pipe closed, a signal), not a `cancel` request.
static ENDING: AtomicBool = AtomicBool::new(false);

/// Ask the running work to stop at its next check. Whoever waits (`nap`, `hold`) is woken.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    ring();
}

/// The same, when the process ends (a download that was paused keeps its part then: `ending`).
pub fn cancel_at_the_end() {
    ENDING.store(true, Ordering::SeqCst);
    cancel();
}

/// The stop came from the end of the process, not from a request.
pub fn ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}

/// Before a new request: nothing is asked to stop, nothing is paused.
pub fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
    ENDING.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
}

pub fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------- the pause, and waiting without polling

/// A `pause` request holds the running request's transfer (`serve`, README "Downloads on a mobile network").
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Taken by a pause, a resume and every `count_live`: after a pause was said, nothing is counted.
static HOLD: Mutex<()> = Mutex::new(());
/// The waiters' lock and bell. Held only to wait and to ring (never while a line is written), so that a cancel
/// never waits behind a writer.
static BELL_LOCK: Mutex<()> = Mutex::new(());
static BELL: Condvar = Condvar::new();

/// Wake every thread that waits in `nap`, `hold` or `wait_bell` (a cancel, a pause, a resume, a worker that ended).
pub fn ring() {
    let _lock = BELL_LOCK.lock().unwrap();
    BELL.notify_all();
}

/// Wait for the bell, `time` at most; not at all when `ready` says so already (asked under the bell's lock: a ring
/// that follows the change `ready` looks at is not missed).
pub fn wait_bell(time: Duration, ready: impl Fn() -> bool) {
    let lock = BELL_LOCK.lock().unwrap();
    if !ready() {
        let _ = BELL.wait_timeout(lock, time).unwrap();
    }
}

/// How many times a paused request was resumed (who keeps a count per pause looks at this).
pub fn resumes() -> u64 {
    RESUMES.load(Ordering::SeqCst)
}

static RESUMES: AtomicU64 = AtomicU64::new(0);

/// Hold the running request's transfer. `said` gets the current step's numbers while nothing can be counted
/// (`count_live` waits), so what it says stays true until the resume.
pub fn pause(said: impl FnOnce(Option<Snapshot>)) {
    {
        let _hold = HOLD.lock().unwrap();
        PAUSED.store(true, Ordering::SeqCst);
        said(current_snapshot());
    }
    ring();
}

/// Let it go on. False, and nothing said, when it was not paused.
pub fn resume(said: impl FnOnce(Option<Snapshot>)) -> bool {
    {
        let _hold = HOLD.lock().unwrap();
        if !PAUSED.swap(false, Ordering::SeqCst) {
            return false;
        }
        RESUMES.fetch_add(1, Ordering::SeqCst);
        said(current_snapshot());
    }
    ring();
    true
}

pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// The request was paused when the end of the process stopped it (the app went away while the user had a download
/// on hold): its part is kept for a later run, as with `"resume":true`.
pub fn held_at_the_end() -> bool {
    paused() && ending()
}

/// `n` more units of the current step, unless the request is paused or stopped: then nothing is counted and the
/// caller drops what it has (false). `counted` runs with the count, before a pause can be said: what the caller
/// keeps of its own (the list of the bytes that are in a part) then agrees with the count a pause says.
pub fn count_live(n: u64, counted: impl FnOnce()) -> bool {
    let _hold = HOLD.lock().unwrap();
    if paused() || cancelled() {
        return false;
    }
    counted();
    count(n);
    true
}

/// Wait while the request is paused (at once `Ok(false)` when it is not); `Ok(true)`: it was held. A stop ends the
/// wait. The caller has nothing in flight: a connection is closed before this is called.
pub fn hold() -> Result<bool, Stop> {
    let mut held = false;
    loop {
        check()?;
        let lock = BELL_LOCK.lock().unwrap();
        if cancelled() {
            return Err(Stop::Cancelled);
        }
        if !paused() {
            return Ok(held);
        }
        held = true;
        // (a second at most: the heat and battery limits are looked at while it waits)
        let _ = BELL.wait_timeout(lock, Duration::from_secs(1)).unwrap();
    }
}

/// Sleep on the bell until `time` is over. It ends early with `Ok` when a pause came (the caller then holds), and
/// with `Err` at once when a stop came: no polling in short steps.
pub fn nap(time: Duration) -> Result<(), Stop> {
    let end = Instant::now() + time;
    loop {
        check()?;
        let lock = BELL_LOCK.lock().unwrap();
        if cancelled() {
            return Err(Stop::Cancelled);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || paused() {
            return Ok(());
        }
        let _ = BELL.wait_timeout(lock, left.min(Duration::from_secs(1))).unwrap();
    }
}

/// The stop a failed step was ended by, when it was one (`check` remembers a heat or battery stop; a cancel is the
/// flag). A step that carries its own error type through says "stopped"; this is the reason behind it.
pub fn stop_reason() -> Option<Stop> {
    if let Some(stop) = gate().lock().unwrap().stopped.clone() {
        return Some(stop);
    }
    cancelled().then_some(Stop::Cancelled)
}

// ---------------------------------------------------------------- threads and nice

static JOBS: AtomicUsize = AtomicUsize::new(0);
/// The most worker threads a request may have.
pub const JOBS_MAX: usize = 8;

/// The online CPUs, at most `JOBS_MAX`.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, JOBS_MAX)
}

/// How many threads work that splits may use: what `set_jobs` said, else `default_jobs`.
pub fn jobs() -> usize {
    match JOBS.load(Ordering::Relaxed) {
        0 => default_jobs(),
        n => n,
    }
}

/// 1 to `JOBS_MAX` (0: back to the default).
pub fn set_jobs(n: usize) {
    JOBS.store(n.min(JOBS_MAX), Ordering::Relaxed);
}

/// Whether `set_jobs` named a count (the one-shot commands keep rayon's own default without it).
pub fn jobs_given() -> bool {
    JOBS.load(Ordering::Relaxed) != 0
}

/// The time-sharing class every process starts in (bionic's headers, and so the libc crate for Android, call it
/// SCHED_NORMAL).
#[cfg(target_os = "android")]
const SCHED_OTHER: libc::c_int = libc::SCHED_NORMAL;
#[cfg(target_os = "linux")]
const SCHED_OTHER: libc::c_int = libc::SCHED_OTHER;

/// Put the process at nice `n` (10 to 19) and out of any realtime class. Call it before the first thread starts:
/// on Linux a nice value is a thread's, and a new thread takes its maker's. Returns what was done, for a log line.
pub fn set_nice(n: i32) -> std::io::Result<String> {
    if !(10..=19).contains(&n) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("nice {n}: 10 to 19")));
    }
    let mut said = String::new();
    // Safety: plain system calls on this process; no pointer but the zeroed parameter block.
    unsafe {
        // Linux's sched_getscheduler/sched_setscheduler have no macOS libc equivalent.
        // On other hosts set_nice still sets the nice value; there is no realtime class to leave here.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let policy = libc::sched_getscheduler(0);
            if policy == libc::SCHED_FIFO || policy == libc::SCHED_RR {
                let param: libc::sched_param = std::mem::zeroed();
                if libc::sched_setscheduler(0, SCHED_OTHER, &param) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                said.push_str("left the realtime class it was started in; ");
            }
        }
        if libc::setpriority(libc::PRIO_PROCESS as _, 0, n) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    said.push_str(&format!("nice {n}"));
    Ok(said)
}

/// This thread's nice value and whether its class is a realtime one (for the tests and `hello`).
pub fn nice_now() -> (i32, bool) {
    // Safety: plain system calls. getpriority's -1 may be a value: errno tells, and a failure reads as 0 here.
    unsafe {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let realtime = {
            let policy = libc::sched_getscheduler(0);
            policy == libc::SCHED_FIFO || policy == libc::SCHED_RR
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let realtime = false;
        (libc::getpriority(libc::PRIO_PROCESS as _, 0), realtime)
    }
}

// ---------------------------------------------------------------- who listens

/// One look at a step's progress.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub step: String,
    pub unit: &'static str,
    pub done: u64,
    /// 0: not known.
    pub total: u64,
    pub items_done: u64,
    /// 0: no count of items.
    pub items_total: u64,
    /// Units per second, once a second of work was measured.
    pub rate: Option<f64>,
    /// Whole seconds left in this step.
    pub eta: Option<u64>,
}

/// What `serve` shows the app. Every method may be called from any worker thread.
pub trait Sink: Send + Sync {
    fn step(&self, step: &str);
    fn progress(&self, snapshot: &Snapshot);
    /// One item of a result: its kind and its fields, in order.
    fn row(&self, kind: &str, fields: &[(&str, serde_json::Value)]);
    fn heat(&self, state: &str, why: &str, text: &str, reading: &Reading);
    /// An event of the request with a name of its own (`net`: the network is waited for, or back), its fields in
    /// order.
    fn event(&self, _name: &str, _fields: &[(&str, serde_json::Value)]) {}
}

static SINK: RwLock<Option<Arc<dyn Sink>>> = RwLock::new(None);

pub fn set_sink(sink: Option<Arc<dyn Sink>>) {
    *SINK.write().unwrap() = sink;
}

fn sink() -> Option<Arc<dyn Sink>> {
    SINK.read().unwrap().clone()
}

/// A step of the running request begins (one of the names its `start` event listed).
pub fn step(name: &str) {
    if let Some(sink) = sink() {
        sink.step(name);
    }
}

/// One item of the running request's result (a file and its sum, a boot, a CPU): a `row` line for the app. Nothing
/// without a listener; the command prints its text as before either way.
pub fn row(kind: &str, fields: &[(&str, serde_json::Value)]) {
    if let Some(sink) = sink() {
        sink.row(kind, fields);
    }
}

/// Whether someone listens (`serve`): a command may then leave out text that its rows carry.
pub fn listening() -> bool {
    SINK.read().unwrap().is_some()
}

/// An event of the running request with a name of its own. False without a listener: the caller then prints its
/// text as a line.
pub fn event(name: &str, fields: &[(&str, serde_json::Value)]) -> bool {
    match sink() {
        Some(sink) => {
            sink.event(name, fields);
            true
        }
        None => false,
    }
}

/// The numbers of the step that counts now.
pub fn current_snapshot() -> Option<Snapshot> {
    CURRENT.read().unwrap().as_ref().map(Progress::snapshot)
}

/// The current step's count starts again at `done` (a stream that can only begin at its first byte).
pub fn recount(done: u64) {
    if let Some(p) = CURRENT.read().unwrap().as_ref() {
        p.set(done);
    }
}

// The step whose bytes the low-level loops count (a hash pass, a copy), set by the request's handler.
static CURRENT: RwLock<Option<Progress>> = RwLock::new(None);
static CURRENT_ON: AtomicBool = AtomicBool::new(false);

/// Begin a step: its `step` line, and a progress that `count` adds to until `end` (or the next `begin`).
pub fn begin(step_name: &str, unit: &'static str, total: u64) -> Progress {
    step(step_name);
    let p = Progress::new(step_name, unit, total);
    *CURRENT.write().unwrap() = Some(p.clone());
    CURRENT_ON.store(true, Ordering::SeqCst);
    p
}

/// The step's last progress line; `count` adds to nothing from now on.
pub fn end() {
    CURRENT_ON.store(false, Ordering::SeqCst);
    if let Some(p) = CURRENT.write().unwrap().take() {
        p.finish();
    }
}

/// `n` more units of the current step are done (bytes hashed, copied, fetched). One atomic load when no step
/// counts.
pub fn count(n: u64) {
    if CURRENT_ON.load(Ordering::Relaxed) {
        if let Some(p) = CURRENT.read().unwrap().as_ref() {
            p.add(n);
        }
    }
}

/// One more item (a file) of the current step is done.
pub fn count_item() {
    if CURRENT_ON.load(Ordering::Relaxed) {
        if let Some(p) = CURRENT.read().unwrap().as_ref() {
            p.item();
        }
    }
}

fn clock() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    clock().elapsed().as_millis() as u64
}

/// The least time between two `progress` lines of a step.
const PROGRESS_EVERY_MS: u64 = 250;

struct Rate {
    at_ms: u64,
    done: u64,
    rate: f64,
}

struct Inner {
    step: String,
    unit: &'static str,
    total: AtomicU64,
    done: AtomicU64,
    items_total: AtomicU64,
    items_done: AtomicU64,
    started_ms: u64,
    next_ms: AtomicU64,
    rate: Mutex<Rate>,
}

/// The progress of one step: counters that any worker thread adds to. A line goes to the listener at the start, at
/// the end (`finish`) and at most four times a second between.
#[derive(Clone)]
pub struct Progress {
    inner: Arc<Inner>,
}

impl Progress {
    /// `total` 0: not known yet (`set_total` later, or never).
    pub fn new(step: &str, unit: &'static str, total: u64) -> Progress {
        let now = now_ms();
        let p = Progress {
            inner: Arc::new(Inner {
                step: step.to_string(),
                unit,
                total: AtomicU64::new(total),
                done: AtomicU64::new(0),
                items_total: AtomicU64::new(0),
                items_done: AtomicU64::new(0),
                started_ms: now,
                next_ms: AtomicU64::new(now + PROGRESS_EVERY_MS),
                rate: Mutex::new(Rate { at_ms: now, done: 0, rate: 0.0 }),
            }),
        };
        p.emit();
        p
    }

    /// With a count of items (files) beside the bytes.
    pub fn items(self, total: u64) -> Progress {
        self.inner.items_total.store(total, Ordering::Relaxed);
        self
    }

    pub fn set_total(&self, total: u64) {
        self.inner.total.store(total, Ordering::Relaxed);
    }

    pub fn add(&self, n: u64) {
        self.inner.done.fetch_add(n, Ordering::Relaxed);
        self.tick();
    }

    pub fn set(&self, done: u64) {
        self.inner.done.store(done, Ordering::Relaxed);
        self.tick();
    }

    /// One more item is done.
    pub fn item(&self) {
        self.inner.items_done.fetch_add(1, Ordering::Relaxed);
        self.tick();
    }

    pub fn done(&self) -> u64 {
        self.inner.done.load(Ordering::Relaxed)
    }

    fn tick(&self) {
        let now = now_ms();
        let next = self.inner.next_ms.load(Ordering::Relaxed);
        // one thread wins the slot and says the line
        if now >= next && self.inner.next_ms.compare_exchange(next, now + PROGRESS_EVERY_MS, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            self.emit();
        }
    }

    /// The step's last line.
    pub fn finish(&self) {
        self.emit();
    }

    pub fn snapshot(&self) -> Snapshot {
        let i = &self.inner;
        let (done, total, now) = (i.done.load(Ordering::Relaxed), i.total.load(Ordering::Relaxed), now_ms());
        let mut rate = i.rate.lock().unwrap();
        let since = now.saturating_sub(rate.at_ms);
        if since >= PROGRESS_EVERY_MS {
            let fresh = done.saturating_sub(rate.done) as f64 * 1000.0 / since as f64;
            // the first measure is the mean so far; later ones lean on the last seconds
            rate.rate = if rate.rate == 0.0 { fresh } else { 0.7 * rate.rate + 0.3 * fresh };
            rate.at_ms = now;
            rate.done = done;
        }
        let measured = now.saturating_sub(i.started_ms) >= 1000 && rate.rate > 0.0;
        Snapshot {
            step: i.step.clone(),
            unit: i.unit,
            done,
            total,
            items_done: i.items_done.load(Ordering::Relaxed),
            items_total: i.items_total.load(Ordering::Relaxed),
            rate: measured.then_some(rate.rate),
            eta: (measured && total > done).then(|| ((total - done) as f64 / rate.rate).ceil() as u64),
        }
    }

    fn emit(&self) {
        if let Some(sink) = sink() {
            sink.progress(&self.snapshot());
        }
    }
}

// ---------------------------------------------------------------- temporary files

static TEMPS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static REMOVED: AtomicUsize = AtomicUsize::new(0);

fn remove_path(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).is_ok(),
        Ok(_) => std::fs::remove_file(path).is_ok(),
        Err(_) => false,
    }
}

/// A file or a folder that exists only while the work runs: removed when this is dropped (the work ended early,
/// by an error, a cancel or a stop), unless `keep` said it is the result now.
pub struct Temp {
    path: PathBuf,
    armed: bool,
}

/// Register `path` as temporary. It need not exist yet.
pub fn temp(path: impl Into<PathBuf>) -> Temp {
    let path = path.into();
    TEMPS.lock().unwrap().push(path.clone());
    Temp { path, armed: true }
}

impl Temp {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path is no longer temporary (it was renamed into place, or it is kept for a later run).
    pub fn keep(mut self) {
        self.armed = false;
        self.forget();
    }

    fn forget(&self) {
        let mut temps = TEMPS.lock().unwrap();
        if let Some(at) = temps.iter().position(|p| p == &self.path) {
            temps.swap_remove(at);
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if self.armed {
            self.forget();
            if remove_path(&self.path) {
                REMOVED.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

/// Remove every temporary path still registered (the process ends without waiting for a request that does not
/// stop). Returns how many were there and removed.
pub fn remove_temps() -> usize {
    let paths: Vec<PathBuf> = std::mem::take(&mut *TEMPS.lock().unwrap());
    let removed = paths.iter().filter(|p| remove_path(p)).count();
    REMOVED.fetch_add(removed, Ordering::SeqCst);
    removed
}

/// How many temporary paths were removed since the last call.
pub fn take_removed() -> usize {
    REMOVED.swap(0, Ordering::SeqCst)
}

// ---------------------------------------------------------------- heat and battery

/// One reading of the named files. A file that is missing or implausible is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The case, degrees C: `battery/lrp` (Samsung's case sensor), else `sec-ap-thermistor/temperature`.
    pub case: Option<f64>,
    /// The cell, degrees C: `battery/temp`.
    pub battery: Option<f64>,
    /// `battery/capacity`, percent.
    pub level: Option<u32>,
    /// `battery/status` is `Charging` or `Full`.
    pub charging: Option<bool>,
}

/// The files, below a root ("" on the phone).
pub const CASE_FILE: &str = "/sys/class/power_supply/battery/lrp";
pub const CASE_FILE_2: &str = "/sys/class/sec/sec-ap-thermistor/temperature";
pub const BATTERY_TEMP_FILE: &str = "/sys/class/power_supply/battery/temp";
pub const BATTERY_LEVEL_FILE: &str = "/sys/class/power_supply/battery/capacity";
pub const BATTERY_STATUS_FILE: &str = "/sys/class/power_supply/battery/status";

fn read_word(root: &str, rel: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("{root}{rel}")).ok()?;
    Some(text.trim().to_string())
}

/// Tenths of a degree as the bridge's `read_deci` takes them: outside -40 to 200 C is no reading.
fn read_deci(root: &str, rel: &str) -> Option<f64> {
    let v: i64 = read_word(root, rel)?.parse().ok()?;
    (-400..=2000).contains(&v).then_some(v as f64 / 10.0)
}

pub fn read_heat(root: &str) -> Reading {
    Reading {
        case: read_deci(root, CASE_FILE).or_else(|| read_deci(root, CASE_FILE_2)),
        battery: read_deci(root, BATTERY_TEMP_FILE),
        level: read_word(root, BATTERY_LEVEL_FILE).and_then(|w| w.parse().ok()).filter(|l| *l <= 100),
        charging: read_word(root, BATTERY_STATUS_FILE).map(|s| s == "Charging" || s == "Full"),
    }
}

/// The limits a heavy request keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Prefixed to the files' paths ("" on the phone; a folder of stand-in files in tests).
    pub root: String,
    /// Degrees C: the case's pause and stop, the battery's pause and stop. None: no heat check.
    pub thermal: Option<[f64; 4]>,
    /// Percent; below it and not charging the work stops. 0: no check.
    pub battery_min: u32,
}

/// A pause ends this far below its limit (the bridge's `skin_hyst` and `battery_hyst`: 2.0 C).
pub const HYSTERESIS: f64 = 2.0;
/// The bridge's HEAVY and TRAPPING levels of the case and of the battery.
pub const THERMAL_DEFAULT: [f64; 4] = [44.0, 46.0, 45.0, 50.0];
pub const BATTERY_MIN_DEFAULT: u32 = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Go,
    /// (`case` or `battery`, the reason in plain words)
    Pause(&'static str, String),
    Stop(Stop),
}

/// What a reading means. `paused`: the work is paused by this sensor now, so it goes on only below the limit less
/// the hysteresis.
pub fn verdict(limits: &Limits, r: &Reading, paused: Option<&str>) -> Verdict {
    if let Some([case_pause, case_stop, battery_pause, battery_stop]) = limits.thermal {
        for (why, name, value, pause, stop) in [("case", "The phone's case", r.case, case_pause, case_stop), ("battery", "The battery", r.battery, battery_pause, battery_stop)] {
            let Some(value) = value else { continue };
            if value >= stop {
                return Verdict::Stop(Stop::Heat(format!("{name} is at {value:.1} C (the limit is {stop:.1} C): stopped. Let the phone cool down, then start again")));
            }
            let limit = if paused == Some(why) { pause - HYSTERESIS } else { pause };
            if value >= limit {
                return Verdict::Pause(why, format!("{name} is at {value:.1} C (the limit is {pause:.1} C): paused until it is below {:.1} C", pause - HYSTERESIS));
            }
        }
    }
    if let (true, Some(level), Some(false)) = (limits.battery_min > 0, r.level, r.charging) {
        if level < limits.battery_min {
            return Verdict::Stop(Stop::Battery(format!("The battery is at {level} % and not charging (the limit is {} %): connect the charger, then start again", limits.battery_min)));
        }
    }
    Verdict::Go
}

#[derive(Default)]
struct Gate {
    limits: Option<Limits>,
    paused: Option<&'static str>,
    stopped: Option<Stop>,
}

fn gate() -> &'static Mutex<Gate> {
    static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
    GATE.get_or_init(Default::default)
}

static GATE_ON: AtomicBool = AtomicBool::new(false);
static GATE_NEXT_MS: AtomicU64 = AtomicU64::new(0);
/// The files are read at most this often.
const GATE_EVERY_MS: u64 = 2000;

/// The limits of the request that begins (None: no check, a light request or a PC). Clears an earlier stop.
pub fn set_limits(limits: Option<Limits>) {
    let mut g = gate().lock().unwrap();
    let on = limits.as_ref().is_some_and(|l| l.thermal.is_some() || l.battery_min > 0);
    *g = Gate { limits, paused: None, stopped: None };
    GATE_NEXT_MS.store(0, Ordering::SeqCst);
    GATE_ON.store(on, Ordering::SeqCst);
}

/// The worker threads' check, between chunks, files, kexts and steps. Returns at once when nothing is asked (two
/// atomic loads). When the limits are on, one thread reads the files every 2 seconds; at a pause limit every thread
/// that checks waits here until the reading is below it, or a cancel comes.
pub fn check() -> Result<(), Stop> {
    if cancelled() {
        return Err(Stop::Cancelled);
    }
    if !GATE_ON.load(Ordering::Relaxed) {
        return Ok(());
    }
    loop {
        let now = now_ms();
        let next = GATE_NEXT_MS.load(Ordering::Relaxed);
        let mine = now >= next && GATE_NEXT_MS.compare_exchange(next, now + GATE_EVERY_MS, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        let mut said: Option<(&'static str, &'static str, String, Reading)> = None;
        let paused = {
            let mut g = gate().lock().unwrap();
            if let Some(stop) = &g.stopped {
                return Err(stop.clone());
            }
            if mine {
                if let Some(limits) = g.limits.clone() {
                    let reading = read_heat(&limits.root);
                    match verdict(&limits, &reading, g.paused) {
                        Verdict::Go => {
                            if let Some(why) = g.paused.take() {
                                said = Some(("resumed", why, "Cool enough again: going on".to_string(), reading));
                            }
                        }
                        Verdict::Pause(why, text) => {
                            if g.paused.is_none() {
                                said = Some(("paused", why, text, reading));
                            }
                            g.paused = Some(why);
                        }
                        Verdict::Stop(stop) => {
                            g.stopped = Some(stop.clone());
                            return Err(stop);
                        }
                    }
                }
            }
            g.paused.is_some()
        };
        if let (Some((state, why, text, reading)), Some(sink)) = (said, sink()) {
            sink.heat(state, why, &text, &reading);
        }
        if !paused {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
        if cancelled() {
            return Err(Stop::Cancelled);
        }
    }
}

/// Sleep, in short steps, until the time is over; an early `Err` when a stop came.
pub fn sleep(time: Duration) -> Result<(), Stop> {
    let end = Instant::now() + time;
    loop {
        check()?;
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state here is the process's: the tests that change it take this lock.
    pub(crate) static STATE: Mutex<()> = Mutex::new(());

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel.trim_start_matches('/'));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn sys(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oneios-job-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_reading_is_the_bridges_files() {
        let root = sys("reading");
        let r = root.to_str().unwrap();
        assert_eq!(read_heat(r), Reading::default(), "a PC: nothing to read");
        put(&root, CASE_FILE_2, "362\n");
        put(&root, BATTERY_TEMP_FILE, "331\n");
        put(&root, BATTERY_LEVEL_FILE, "57\n");
        put(&root, BATTERY_STATUS_FILE, "Discharging\n");
        assert_eq!(read_heat(r), Reading { case: Some(36.2), battery: Some(33.1), level: Some(57), charging: Some(false) });
        // lrp, Samsung's case sensor, comes before the board thermistor
        put(&root, CASE_FILE, "401\n");
        assert_eq!(read_heat(r).case, Some(40.1));
        // a sensor that is off reads -273 C: no reading, as the bridge's read_deci has it
        put(&root, CASE_FILE, "-2730\n");
        assert_eq!(read_heat(r).case, Some(36.2));
        put(&root, BATTERY_STATUS_FILE, "Full\n");
        assert_eq!(read_heat(r).charging, Some(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_limits_pause_stop_and_hold() {
        let limits = Limits { root: String::new(), thermal: Some(THERMAL_DEFAULT), battery_min: 10 };
        let at = |case: f64, battery: f64| Reading { case: Some(case), battery: Some(battery), level: Some(80), charging: Some(false) };
        assert_eq!(verdict(&limits, &at(43.9, 30.0), None), Verdict::Go);
        assert!(matches!(verdict(&limits, &at(44.0, 30.0), None), Verdict::Pause("case", text) if text.contains("44.0 C") && text.contains("below 42.0 C")));
        // paused: 43.0 C holds (off below 42), 41.9 C goes on
        assert!(matches!(verdict(&limits, &at(43.0, 30.0), Some("case")), Verdict::Pause("case", _)));
        assert_eq!(verdict(&limits, &at(41.9, 30.0), Some("case")), Verdict::Go);
        assert!(matches!(verdict(&limits, &at(46.0, 30.0), None), Verdict::Stop(Stop::Heat(text)) if text.contains("46.0 C")));
        assert!(matches!(verdict(&limits, &at(30.0, 45.2), None), Verdict::Pause("battery", _)));
        assert!(matches!(verdict(&limits, &at(30.0, 50.0), None), Verdict::Stop(Stop::Heat(text)) if text.starts_with("The battery")));
        // the battery's level counts only when it is not charging
        let low = Reading { case: Some(30.0), battery: Some(30.0), level: Some(9), charging: Some(false) };
        assert!(matches!(verdict(&limits, &low, None), Verdict::Stop(Stop::Battery(text)) if text.contains("9 %")));
        assert_eq!(verdict(&limits, &Reading { charging: Some(true), ..low.clone() }, None), Verdict::Go);
        assert_eq!(verdict(&Limits { battery_min: 0, ..limits.clone() }, &low, None), Verdict::Go);
        // nothing to read (a PC), or the check is off
        assert_eq!(verdict(&limits, &Reading::default(), None), Verdict::Go);
        assert_eq!(verdict(&Limits { thermal: None, ..limits }, &at(60.0, 60.0), None), Verdict::Go);
    }

    #[test]
    fn check_pauses_until_cool_and_stops_at_the_limit() {
        let _state = STATE.lock().unwrap();
        let root = sys("gate");
        put(&root, CASE_FILE, "445\n");
        put(&root, BATTERY_TEMP_FILE, "300\n");
        clear_cancel();
        set_limits(Some(Limits { root: root.to_str().unwrap().to_string(), thermal: Some(THERMAL_DEFAULT), battery_min: 0 }));
        let cool = root.clone();
        let cooler = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            put(&cool, CASE_FILE, "410\n");
        });
        let began = Instant::now();
        assert_eq!(check(), Ok(()), "the pause ends when the case is below 42 C");
        // the files are read every 2 s: the pause lasts until the read after the case cooled
        assert!(began.elapsed() >= Duration::from_millis(1500), "it waited: {:?}", began.elapsed());
        cooler.join().unwrap();
        put(&root, CASE_FILE, "461\n");
        GATE_NEXT_MS.store(0, Ordering::SeqCst);
        assert!(matches!(check(), Err(Stop::Heat(_))));
        // the stop stays for every later check of this request
        put(&root, CASE_FILE, "300\n");
        assert!(matches!(check(), Err(Stop::Heat(_))));
        assert!(matches!(stop_reason(), Some(Stop::Heat(_))));
        set_limits(None);
        assert_eq!(check(), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancel_ends_a_pause_and_a_sleep() {
        let _state = STATE.lock().unwrap();
        let root = sys("cancel");
        put(&root, CASE_FILE, "450\n");
        clear_cancel();
        set_limits(Some(Limits { root: root.to_str().unwrap().to_string(), thermal: Some(THERMAL_DEFAULT), battery_min: 0 }));
        let stopper = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            cancel();
        });
        assert_eq!(check(), Err(Stop::Cancelled));
        stopper.join().unwrap();
        set_limits(None);
        assert_eq!(sleep(Duration::from_secs(30)), Err(Stop::Cancelled));
        clear_cancel();
        assert_eq!(sleep(Duration::from_millis(10)), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pause_holds_until_the_resume_and_nothing_is_counted_after_it() {
        let _state = STATE.lock().unwrap();
        clear_cancel();
        set_limits(None);
        assert_eq!(hold(), Ok(false), "not paused: no wait");
        let progress = begin("download", "bytes", 100);
        assert!(count_live(10, || ()));
        let mut said = None;
        pause(|now| said = now.map(|s| (s.step, s.done, s.total)));
        assert_eq!(said, Some(("download".to_string(), 10, 100)), "the pause says the step's numbers");
        assert!(paused() && !count_live(5, || panic!("nothing is listed after a pause")));
        assert_eq!(progress.done(), 10);
        // a thread that holds wakes at the resume, and knows that it was held
        let waiter = std::thread::spawn(|| {
            let began = Instant::now();
            (hold(), began.elapsed())
        });
        std::thread::sleep(Duration::from_millis(150));
        let before = resumes();
        assert!(resume(|now| assert_eq!(now.map(|s| s.done), Some(10))));
        assert!(!resume(|_| panic!("it is not paused")));
        let (held, took) = waiter.join().unwrap();
        assert_eq!(held, Ok(true));
        assert!(took >= Duration::from_millis(100) && took < Duration::from_secs(1), "the resume's bell, not a poll: {took:?}");
        assert_eq!(resumes(), before + 1);
        assert!(count_live(5, || ()));
        assert_eq!(progress.done(), 15);
        end();
        // the end of the process stops a paused request: its hold ends, and its part is one to keep
        pause(|now| assert_eq!(now, None, "no step counts"));
        let waiter = std::thread::spawn(hold);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!held_at_the_end());
        cancel_at_the_end();
        assert_eq!(waiter.join().unwrap(), Err(Stop::Cancelled));
        assert!(held_at_the_end() && !count_live(1, || ()));
        clear_cancel();
        assert!(!paused() && !ending() && !held_at_the_end());
        // a nap ends at once at a cancel (an error) and at a pause (the caller then holds), and else when its time is over
        for stop in [true, false] {
            let napper = std::thread::spawn(|| {
                let began = Instant::now();
                (nap(Duration::from_secs(30)), began.elapsed())
            });
            std::thread::sleep(Duration::from_millis(100));
            if stop {
                cancel();
            } else {
                pause(|_| ());
            }
            let (napped, took) = napper.join().unwrap();
            assert_eq!(napped, if stop { Err(Stop::Cancelled) } else { Ok(()) });
            assert!(took < Duration::from_secs(1), "{took:?}");
            clear_cancel();
        }
        let began = Instant::now();
        assert_eq!(nap(Duration::from_millis(60)), Ok(()));
        assert!(began.elapsed() >= Duration::from_millis(60));
        // the bell: no wait when the thing waited for is there already
        let began = Instant::now();
        wait_bell(Duration::from_secs(30), || true);
        assert!(began.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_temporary_path_goes_unless_kept() {
        let _state = STATE.lock().unwrap();
        let dir = sys("temp");
        take_removed();
        let (file, folder, kept) = (dir.join("a.part"), dir.join("b.part"), dir.join("c.part"));
        std::fs::write(&file, b"x").unwrap();
        std::fs::create_dir_all(folder.join("deep")).unwrap();
        std::fs::write(folder.join("deep/f"), b"y").unwrap();
        std::fs::write(&kept, b"z").unwrap();
        let (a, b, c, never) = (temp(&file), temp(&folder), temp(&kept), temp(dir.join("never-made")));
        c.keep();
        drop((a, b, never));
        assert!(!file.exists() && !folder.exists() && kept.exists());
        assert_eq!(take_removed(), 2, "the path that was never made does not count");
        // the forced end: what is still registered
        let late = dir.join("late.part");
        std::fs::write(&late, b"x").unwrap();
        let guard = temp(&late);
        assert_eq!(remove_temps(), 1);
        assert!(!late.exists());
        drop(guard);
        assert_eq!(take_removed(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_counts_and_measures() {
        let p = Progress::new("copy", "bytes", 1000).items(4);
        p.add(250);
        p.item();
        let s = p.snapshot();
        assert_eq!((s.done, s.total, s.items_done, s.items_total, s.unit), (250, 1000, 1, 4, "bytes"));
        assert_eq!((s.rate, s.eta), (None, None), "no rate before a second of work");
        std::thread::sleep(Duration::from_millis(1050));
        p.add(250);
        let s = p.snapshot();
        let rate = s.rate.expect("a rate after a second");
        assert!((100.0..=600.0).contains(&rate), "about 500 units in about a second: {rate}");
        assert!(matches!(s.eta, Some(1..=5)), "500 left at that rate: {:?}", s.eta);
        p.set(1000);
        assert_eq!(p.snapshot().eta, None, "nothing left");
    }

    #[test]
    fn the_thread_count_is_bounded() {
        let _state = STATE.lock().unwrap();
        assert!((1..=JOBS_MAX).contains(&default_jobs()));
        set_jobs(3);
        assert_eq!((jobs(), jobs_given()), (3, true));
        set_jobs(99);
        assert_eq!(jobs(), JOBS_MAX);
        set_jobs(0);
        assert_eq!((jobs(), jobs_given()), (default_jobs(), false));
    }

    #[test]
    fn nice_is_10_to_19_and_never_realtime() {
        assert!(set_nice(9).is_err() && set_nice(20).is_err() && set_nice(-5).is_err());
        // (the test binary's other threads keep their own nice: on Linux it is a thread's)
        let before = nice_now();
        if before.0 <= 12 {
            assert_eq!(set_nice(12).unwrap(), "nice 12");
            assert_eq!(nice_now(), (12, false));
        }
        // a thread started now takes its maker's nice
        let child = std::thread::spawn(nice_now).join().unwrap();
        assert_eq!(child, nice_now());
    }
}
