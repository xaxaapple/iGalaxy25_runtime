//! `ioslog health`: the counts of a boot that tell lag and heat apart, from iOS's own log (the owner's request,
//! 2026-11-15): HangTracer's hangs, watchdog kills, XPC timeouts, jetsam kills, crashes, memorystatus' stuck idle
//! demotions and CMPhoto encode failures. One line per kind: its count, where the lines carry seconds their total
//! and the longest, and the top processes.
//!
//! **A count is a number of log lines.** One event may be logged more than once (RunningBoard's exit status is
//! logged by every process that watches the one that ended), so compare a count with the same count of another
//! session, and read `--detail` for each text's own count.
//!
//! **A new kind is a new row of `KINDS`**, nothing else: its name, the message texts (regular expressions) and
//! how its lines are filed. `tests/ioslog/health.txt` is the fixture the tests count; add the new kind's lines
//! there.
//!
//! The source is what every `ioslog` command reads (the Data store's image, a `.logarchive`, a folder), or a text
//! file that `ioslog show` wrote (`-f text`): the same counter reads both, which is also how the tests feed it
//! (lines written by hand, no Apple file).

use crate::{base_name, open_source, pick_boot, text_line, time_text, uuid_text, How};
use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use macos_unifiedlogs::logarchive::visit_provider;
use regex::{Regex, RegexSet};
use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;

/// Under which name a line is counted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum By {
    /// The process that logged the line (the app that hung, the client whose call timed out).
    Logger,
    /// The process the line names (the kernel logs a kill; the line names the process killed).
    Named,
}

/// One kind of line.
pub struct Kind {
    /// The line's first word.
    pub name: &'static str,
    /// What the count means (`--detail`).
    pub what: &'static str,
    /// (short name, regular expression on the message). A line is counted once per kind, under the first text
    /// that matches. Named groups a text may hold: `s`, a number of seconds (the kind then has a total, a longest
    /// and an "over" count); `who`, the process, extension or service the line names.
    pub texts: &'static [(&'static str, &'static str)],
    pub by: By,
    /// What the names in the lines are (`extensions`, `services`); with `By::Named` they are processes.
    pub names: &'static str,
    /// More expressions with a `who` group, tried in order when the text itself named nothing.
    pub who: &'static [&'static str],
    /// With `s`: lines over this many seconds are counted apart (0: not).
    pub over: f64,
}

/// RunningBoard's name of a process at the start of its lines: `[osservice<com.apple.chronod>:87]`,
/// `[xpcservice<com.apple.weather.widget([osservice<com.apple.chronod>:87])>:120]`.
const RUNNINGBOARD_NAME: &str = r"\[\w+<(?P<who>[^>(\[\]]+)";
/// A reverse-DNS name anywhere in the line (a bundle or a Mach service). A guess at what the line is about: the
/// formats of the watchdog and XPC timeout lines were not at hand when this was written (see README "health").
const DNS_NAME: &str = r"\b(?P<who>(?:com|org|net|io|group)\.[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+)";

/// The kinds, in the order of the report. To add one, add a row (and its lines to tests/ioslog/health.txt).
pub const KINDS: &[Kind] = &[
    Kind {
        name: "hangs",
        what: "HangTracer's `Hang detected: N s`: a main thread that did not answer for N s, by the process that hung",
        texts: &[("Hang detected", r"Hang detected: (?P<s>[0-9]+(?:\.[0-9]+)?) ?s\b")],
        by: By::Logger,
        names: "",
        who: &[],
        over: 1.0,
    },
    Kind {
        name: "watchdog",
        what: "watchdog kills: RunningBoard's exit status of the watchdog domain, `watchdog fired`, and an extension whose termination was watchdogged (chronod's widgets), by the process that logged the line; the names are the processes and extensions the lines name",
        texts: &[
            ("exit status", r"RBSProcessExitStatus\| domain:watchdog"),
            ("extension", r"Extension process termination watchdogged"),
            ("fired", r"(?i)watchdog fired"),
        ],
        by: By::Logger,
        names: "named",
        who: &[RUNNINGBOARD_NAME, DNS_NAME],
        over: 0.0,
    },
    Kind {
        name: "xpc-timeouts",
        what: "XPC calls that timed out (`Initial XPC call to server timed out`, `Default XPC timeout`, `XPC timeout`), by the client that logged the line; the services are the names in the lines",
        texts: &[("initial call", r"Initial XPC call to server timed out"), ("default", r"Default XPC timeout"), ("timeout", r"XPC timeout")],
        by: By::Logger,
        names: "services",
        who: &[DNS_NAME],
        over: 0.0,
    },
    Kind {
        name: "jetsam",
        what: "the kernel's memory kills (`memorystatus: killing...`), by the process killed; `idle exit` is a clean daemon ended while idle, `kill` every other reason",
        texts: &[("idle exit", r"memorystatus: killing_idle_process"), ("kill", r"memorystatus: killing")],
        by: By::Named,
        names: "",
        who: &[r"memorystatus: killing\w* (?:pid |process )?\d+ \[(?P<who>[^\]]+)\]"],
        over: 0.0,
    },
    Kind {
        name: "crashes",
        what: "crashes, as the kernel's corpse lines (`NAME[PID] Corpse allowed N of 5`, `Corpse failure, too many`), by the process that crashed",
        texts: &[("corpse", r"\[\d+\] Corpse allowed \d+ of \d+"), ("too many corpses", r"\[\d+\] Corpse failure, too many")],
        by: By::Named,
        names: "",
        who: &[r"(?P<who>[^\s\[]+)\[\d+\] Corpse "],
        over: 0.0,
    },
    Kind {
        name: "stuck-idle",
        what: "memorystatus' idle demotion that `timed out stuck process PID [NAME]`: a daemon that did not go idle in time, by that process",
        texts: &[("stuck", r"timed out stuck process")],
        by: By::Named,
        names: "",
        who: &[r"stuck process \d+ \[(?P<who>[^\]]+)\]"],
        over: 0.0,
    },
    Kind {
        name: "encode-fail",
        what: "CMPhoto/ImageIO encodes that failed (`CMPhotoCompressionSessionAddImage ... err =`), by the process that logged the line; the errors are what follows `err =`",
        texts: &[("AddImage", r"CMPhotoCompressionSessionAddImage.*\berr ?=")],
        by: By::Logger,
        names: "errors",
        who: &[r"\berr ?= ?(?P<who>[^\s,;)]+(?: \[-?\d+\])?)"],
        over: 0.0,
    },
];

/// A count, and for a kind with seconds their sum, the longest and how many were over the kind's limit.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Tally {
    pub count: u64,
    pub seconds: f64,
    pub longest: f64,
    pub over: u64,
}

impl Tally {
    fn add(&mut self, seconds: Option<f64>, limit: f64) {
        self.count += 1;
        if let Some(s) = seconds {
            self.seconds += s;
            self.longest = self.longest.max(s);
            if limit > 0.0 && s > limit {
                self.over += 1;
            }
        }
    }
}

/// One kind's result.
pub struct Count {
    pub kind: &'static Kind,
    pub all: Tally,
    /// Who logged the longest line (a kind with seconds).
    pub longest_by: String,
    /// The first and the last line's time (nanoseconds since 1970).
    pub first: f64,
    pub last: f64,
    /// Each text's lines, in the order of `kind.texts`.
    pub texts: Vec<u64>,
    /// Per text: the processes that logged its lines, the largest count first.
    pub text_loggers: Vec<Vec<(String, Tally)>>,
    /// By the process that logged the line, the largest count first.
    pub loggers: Vec<(String, Tally)>,
    /// By the name in the line (only the lines that name something), the largest count first.
    pub names: Vec<(String, Tally)>,
    pub examples: Vec<String>,
}

impl Count {
    /// True when the kind's lines carry seconds.
    pub fn timed(&self) -> bool {
        self.kind.texts.iter().any(|(_, text)| text.contains("(?P<s>"))
    }

    /// The list a summary line shows: the loggers, or for `By::Named` the names.
    pub fn top(&self) -> &[(String, Tally)] {
        match self.kind.by {
            By::Logger => &self.loggers,
            By::Named => &self.names,
        }
    }
}

/// What `health` read and counted.
pub struct Report {
    /// The source, as the other commands name it.
    pub source: String,
    /// `boot UUID`, `all boots`, or what a text file holds.
    pub boot: String,
    pub entries: u64,
    pub first: f64,
    pub last: f64,
    pub kinds: Vec<Count>,
}

struct Building {
    all: Tally,
    longest_by: String,
    first: f64,
    last: f64,
    texts: Vec<u64>,
    text_loggers: Vec<HashMap<String, Tally>>,
    loggers: HashMap<String, Tally>,
    names: HashMap<String, Tally>,
    examples: Vec<String>,
}

/// The name for a line of a `By::Named` kind that names nobody.
const NOT_NAMED: &str = "(not named)";

/// Counts entries as they come.
pub struct Counter {
    /// Every text of every kind, for one pass over a message.
    any: RegexSet,
    /// Per text of `any`: its kind, its place in the kind's texts, and the expression itself when it has groups.
    texts: Vec<(usize, usize, Option<Regex>)>,
    who: Vec<Vec<Regex>>,
    building: Vec<Building>,
    examples: usize,
    entries: u64,
    first: f64,
    last: f64,
}

fn sorted(map: HashMap<String, Tally>) -> Vec<(String, Tally)> {
    let mut list: Vec<(String, Tally)> = map.into_iter().collect();
    list.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.seconds.partial_cmp(&a.1.seconds).unwrap_or(std::cmp::Ordering::Equal)).then(a.0.cmp(&b.0)));
    list
}

impl Counter {
    /// `examples`: how many example lines to keep per kind.
    pub fn new(examples: usize) -> Result<Counter> {
        let mut flat = Vec::new();
        let mut texts = Vec::new();
        let mut who = Vec::new();
        for (k, kind) in KINDS.iter().enumerate() {
            for (t, (_, text)) in kind.texts.iter().enumerate() {
                let re = Regex::new(text).with_context(|| format!("health: {}'s text {text}", kind.name))?;
                let groups = re.capture_names().flatten().next().is_some();
                flat.push(*text);
                texts.push((k, t, groups.then_some(re)));
            }
            who.push(kind.who.iter().map(|w| Regex::new(w).with_context(|| format!("health: {}'s name {w}", kind.name))).collect::<Result<Vec<_>>>()?);
        }
        let building = KINDS
            .iter()
            .map(|kind| Building {
                all: Tally::default(),
                longest_by: String::new(),
                first: f64::MAX,
                last: f64::MIN,
                texts: vec![0; kind.texts.len()],
                text_loggers: vec![HashMap::new(); kind.texts.len()],
                loggers: HashMap::new(),
                names: HashMap::new(),
                examples: Vec::new(),
            })
            .collect();
        Ok(Counter { any: RegexSet::new(flat)?, texts, who, building, examples, entries: 0, first: f64::MAX, last: f64::MIN })
    }

    /// One entry: its time (nanoseconds since 1970), the process that logged it (the last part of its path) and
    /// its message. `line` gives the entry as a line of text, for the examples.
    pub fn add(&mut self, time: f64, process: &str, message: &str, line: impl Fn() -> String) {
        self.entries += 1;
        self.first = self.first.min(time);
        self.last = self.last.max(time);
        let mut last_kind = usize::MAX;
        // the texts are in the kinds' order and each kind's own order: the first hit of a kind is its first text
        for hit in self.any.matches(message) {
            let (k, t, groups) = &self.texts[hit];
            if *k == last_kind {
                continue;
            }
            last_kind = *k;
            let kind = &KINDS[*k];
            let found = groups.as_ref().and_then(|re| re.captures(message));
            let seconds = found.as_ref().and_then(|c| c.name("s")).and_then(|s| s.as_str().parse::<f64>().ok());
            let named = found
                .as_ref()
                .and_then(|c| c.name("who"))
                .or_else(|| self.who[*k].iter().find_map(|re| re.captures(message).and_then(|c| c.name("who"))))
                .map(|m| m.as_str().to_string());
            let b = &mut self.building[*k];
            if seconds.is_some_and(|s| s > b.all.longest) {
                b.longest_by = process.to_string();
            }
            b.all.add(seconds, kind.over);
            b.first = b.first.min(time);
            b.last = b.last.max(time);
            b.texts[*t] += 1;
            b.text_loggers[*t].entry(process.to_string()).or_default().add(seconds, kind.over);
            b.loggers.entry(process.to_string()).or_default().add(seconds, kind.over);
            match named {
                Some(name) => b.names.entry(name).or_default().add(seconds, kind.over),
                None if kind.by == By::Named => b.names.entry(NOT_NAMED.to_string()).or_default().add(seconds, kind.over),
                None => {}
            }
            if b.examples.len() < self.examples {
                b.examples.push(line().trim_end().to_string());
            }
        }
    }

    pub fn finish(self, source: String, boot: String) -> Report {
        let kinds = KINDS
            .iter()
            .zip(self.building)
            .map(|(kind, b)| Count {
                kind,
                all: b.all,
                longest_by: b.longest_by,
                first: b.first,
                last: b.last,
                texts: b.texts,
                text_loggers: b.text_loggers.into_iter().map(sorted).collect(),
                loggers: sorted(b.loggers),
                names: sorted(b.names),
                examples: b.examples,
            })
            .collect();
        Report { source, boot, entries: self.entries, first: self.first, last: self.last, kinds }
    }
}

/// `1` for 1.0, `0.5` for 0.5.
fn plain(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

/// `38 (251.1 s, longest 61.27 s, 30 over 1 s)`, or `84` for a kind without seconds.
fn tally_text(tally: &Tally, count: &Count) -> String {
    if !count.timed() {
        return tally.count.to_string();
    }
    let over = if count.kind.over > 0.0 { format!(", {} over {} s", tally.over, plain(count.kind.over)) } else { String::new() };
    format!("{} ({:.1} s, longest {:.2} s{over})", tally.count, tally.seconds, tally.longest)
}

/// `Preferences 38 (...), SpringBoard 25 (...), and 3 more with 9`.
fn list_text(list: &[(String, Tally)], count: &Count, top: usize) -> String {
    let mut parts: Vec<String> = list.iter().take(top).map(|(name, tally)| format!("{name} {}", tally_text(tally, count))).collect();
    if list.len() > top {
        parts.push(format!("and {} more with {}", list.len() - top, list[top..].iter().map(|(_, t)| t.count).sum::<u64>()));
    }
    parts.join(", ")
}

impl Report {
    /// The first line: what was read.
    pub fn head(&self) -> String {
        if self.entries == 0 {
            return format!("{}: {}, 0 entries", self.source, self.boot);
        }
        format!(
            "{}: {}, {} entries, from {} to {} ({:.0} s)",
            self.source,
            self.boot,
            self.entries,
            time_text(self.first),
            time_text(self.last),
            (self.last - self.first) / 1e9
        )
    }

    /// One kind's line: `NAME COUNT  TOTAL, LONGEST, OVER; the top processes; the names in the lines`.
    pub fn line(&self, count: &Count, top: usize) -> String {
        let mut parts = Vec::new();
        if count.all.count > 0 {
            if count.timed() {
                let over = if count.kind.over > 0.0 { format!(", {} over {} s", count.all.over, plain(count.kind.over)) } else { String::new() };
                parts.push(format!("{:.1} s in all, longest {:.2} s ({}){over}", count.all.seconds, count.all.longest, count.longest_by));
            }
            parts.push(list_text(count.top(), count, top));
            if count.kind.by == By::Logger && !count.names.is_empty() {
                parts.push(format!("{}: {}", count.kind.names, list_text(&count.names, count, top)));
            }
        }
        format!("{:<13}{:>7}  {}", count.kind.name, count.all.count, parts.join("; ")).trim_end().to_string()
    }

    /// The kinds' lines, one each.
    pub fn summary(&self, top: usize) -> Vec<String> {
        self.kinds.iter().map(|count| self.line(count, top)).collect()
    }

    /// The report: the head, then each kind's line. With `detail`, under each kind (indented): what it counts,
    /// the first and last time, each text's count, every process and name, and the example lines.
    pub fn text(&self, top: usize, detail: bool) -> String {
        let mut out = self.head() + "\n";
        for count in &self.kinds {
            out += &self.line(count, top);
            out.push('\n');
            if !detail {
                continue;
            }
            out += &format!("    counts: {}\n", count.kind.what);
            if count.all.count == 0 {
                continue;
            }
            out += &format!("    first {}, last {}\n", time_text(count.first), time_text(count.last));
            // a kind of several texts that is filed by the logger: who logged each text (a check against
            // `ioslog show --proc NAME --grep TEXT`)
            let who = count.kind.by == By::Logger && count.kind.texts.len() > 1;
            let texts: Vec<String> = count
                .kind
                .texts
                .iter()
                .zip(count.texts.iter().zip(&count.text_loggers))
                .map(|((name, _), (n, loggers))| {
                    let by: Vec<String> = loggers.iter().map(|(logger, tally)| format!("{logger} {}", tally.count)).collect();
                    if who && *n > 0 {
                        format!("{name} {n} ({})", by.join(", "))
                    } else {
                        format!("{name} {n}")
                    }
                })
                .collect();
            out += &format!("    texts: {}\n", texts.join(", "));
            match count.kind.by {
                By::Logger => {
                    out += &format!("    by process: {}\n", list_text(&count.loggers, count, usize::MAX));
                    if !count.names.is_empty() {
                        out += &format!("    {}: {}\n", count.kind.names, list_text(&count.names, count, usize::MAX));
                    }
                }
                By::Named => {
                    out += &format!("    by process: {}\n", list_text(&count.names, count, usize::MAX));
                    out += &format!("    logged by: {}\n", list_text(&count.loggers, count, usize::MAX));
                }
            }
            for line in &count.examples {
                out += &format!("    e.g. {}\n", line.replace('\n', "\n         "));
            }
        }
        out
    }
}

/// The first line of an entry in `ioslog show`'s text: `TIME PROCESS[PID] TYPE` and the rest.
const TEXT_HEAD: &str = r"^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d(?:\.\d+)?)Z (.*?)\[\d+\] [A-Za-z]+(?: (.*))?$";
/// `[SUBSYSTEM:CATEGORY] ` before the message. RunningBoard's `[osservice<NAME>:PID]` is a message's own start and
/// stays. A message that itself begins with `[word:word] ` in an entry without a subsystem loses that start: the
/// text form cannot tell the two apart.
const TEXT_SCOPE: &str = r"^\[[A-Za-z0-9._-]*:[^\]\[<>]*\] ?";

/// True when the file's first line is an entry of `ioslog show`'s text.
fn is_text(path: &str) -> bool {
    let mut start = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(4096).read_to_end(&mut start));
    if read.is_err() || !Path::new(path).is_file() {
        return false;
    }
    let text = String::from_utf8_lossy(&start);
    Regex::new(TEXT_HEAD).expect("a constant").is_match(text.lines().next().unwrap_or_default())
}

/// Count the entries of `ioslog show`'s text. A line that is no entry's first line belongs to the message of
/// the entry before it.
pub fn count_text(text: impl BufRead, counter: &mut Counter) -> Result<()> {
    let head = Regex::new(TEXT_HEAD)?;
    let scope = Regex::new(TEXT_SCOPE)?;
    // the entry being read: its time, its process, its message, and its lines as they are
    let mut entry: Option<(f64, String, String, String)> = None;
    let flush = |entry: &mut Option<(f64, String, String, String)>, counter: &mut Counter| {
        if let Some((time, process, message, raw)) = entry.take() {
            counter.add(time, &process, &message, || raw.clone());
        }
    };
    for line in text.split(b'\n') {
        let line = line?;
        let line = String::from_utf8_lossy(&line);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        let first = head.captures(line).and_then(|c| {
            let time = NaiveDateTime::parse_from_str(&c[1], "%Y-%m-%d %H:%M:%S%.f").ok()?.and_utc().timestamp_nanos_opt()?;
            let rest = c.get(3).map(|m| m.as_str()).unwrap_or_default();
            // the text has the time cut to the microsecond: half of one more, so that it is written as it was read
            // (the times are f64 nanoseconds, in steps of 256 ns at these dates)
            Some((time as f64 + 500.0, c[2].to_string(), scope.replace(rest, "").into_owned()))
        });
        match (first, &mut entry) {
            (Some((time, process, message)), _) => {
                flush(&mut entry, counter);
                entry = Some((time, process, message, line.to_string()));
            }
            (None, Some((_, _, message, raw))) => {
                message.push('\n');
                message.push_str(line);
                raw.push('\n');
                raw.push_str(line);
            }
            // lines before the first entry: not `show`'s
            (None, None) => {}
        }
    }
    flush(&mut entry, counter);
    Ok(())
}

/// Read `source` and count: an image, a `.logarchive` or a folder (one boot of it: `last`, `all`, or the start of
/// a UUID), or a text file of `ioslog show` (every entry of it).
pub fn read(source: &str, how: &How, boot: &str, examples: usize) -> Result<Report> {
    let mut counter = Counter::new(examples)?;
    if is_text(source) {
        if boot != "last" && boot != "all" {
            bail!("{source}: a text file names no boots; choose the boot when the file is written (ioslog show --boot {boot})");
        }
        let file = std::fs::File::open(source).with_context(|| source.to_string())?;
        count_text(std::io::BufReader::with_capacity(1 << 20, file), &mut counter)?;
        return Ok(counter.finish(source.to_string(), "the entries of a text file (ioslog show's)".to_string()));
    }
    let log = open_source(source, how)?;
    let boot = pick_boot(&log, boot)?;
    visit_provider(&log, |entry| {
        if boot.is_some_and(|b| b != entry.boot_uuid) {
            return;
        }
        let message = entry.message();
        counter.add(entry.time, base_name(entry.process.unwrap_or("?")), &message, || text_line(&entry));
    })?;
    let boot = match boot {
        Some(b) => format!("boot {}", uuid_text(b)),
        None => "all boots".to_string(),
    };
    Ok(counter.finish(log.what.clone(), boot))
}

/// How many processes a line lists.
pub const TOP: usize = 5;
/// How many example lines a kind gets in the file.
pub const EXAMPLES: usize = 3;

/// For `cycle` and `phone pull-data`: count `source`'s last boot, write the report with its details to `out`,
/// and return the head and the kinds' lines.
pub fn write(source: &str, out: &Path) -> Result<Vec<String>> {
    let report = read(source, &How::default(), "last", EXAMPLES)?;
    std::fs::write(out, report.text(TOP, true)).with_context(|| out.display().to_string())?;
    let mut lines = vec![report.head()];
    lines.extend(report.summary(TOP));
    Ok(lines)
}

/// The kinds' lines of a report that `write` (or `--output`) wrote: its lines that are not indented, without
/// the head.
pub fn summary_of(report: &str) -> Vec<String> {
    report.lines().skip(1).filter(|line| !line.starts_with(' ') && !line.is_empty()).map(str::to_string).collect()
}

#[derive(clap::Args)]
pub struct Args {
    /// The Data store's image, a .logarchive folder, a folder with db/diagnostics, or a text file that `ioslog
    /// show` wrote. Without it: STAGE/data/data.img, the staged Data store (the form for the phone itself)
    source: Option<String>,
    #[command(flatten)]
    how: How,
    /// The boot to count: last (default), all, or the start of a boot UUID
    #[arg(long, default_value = "last")]
    boot: String,
    /// How many processes (and names) a kind's line lists
    #[arg(long, default_value_t = TOP)]
    top: usize,
    /// Under each kind: what it counts, the first and last time, each text's count, every process and name, examples
    #[arg(long)]
    detail: bool,
    /// How many example lines per kind (with --detail, and in --output's file)
    #[arg(long, default_value_t = EXAMPLES)]
    examples: usize,
    /// Also write the report with its details to FILE (an existing file is replaced)
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
    /// Without SOURCE: the stage whose data/data.img is read
    #[arg(long, value_name = "DIR", default_value = "/data/local/tmp/oneios")]
    stage: String,
}

/// The staged Data store, for a command without SOURCE. Says on stderr when a session is open: the store is then
/// being written, and what is read is its newest checkpoint that passes the reader's checks.
fn staged(stage: &str) -> Result<String> {
    let image = format!("{stage}/data/data.img");
    if !Path::new(&image).is_file() {
        bail!("no SOURCE, and no staged Data store at {image} (without SOURCE the command reads the stage's own store: the form for the phone itself, as root; --stage DIR names another stage)");
    }
    let pid = std::fs::read_to_string(format!("{stage}/session/xnuhost.pid")).unwrap_or_default();
    let pid = pid.trim();
    if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) {
        let running = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("xnuhost"));
        if running {
            eprintln!("ioslog: a session is open (xnuhost, pid {pid}): the store is being written. The counts are those of the checkpoint that is read; what iOS logged last may not be in it yet");
        }
    }
    Ok(image)
}

pub(crate) fn run(args: Args) -> Result<()> {
    let source = match &args.source {
        Some(source) => source.clone(),
        None => staged(&args.stage)?,
    };
    let report = read(&source, &args.how, &args.boot, args.examples)?;
    print!("{}", report.text(args.top, args.detail));
    if let Some(path) = &args.output {
        std::fs::write(path, report.text(args.top, true)).with_context(|| path.display().to_string())?;
        eprintln!("ioslog: the report with its details is in {}", path.display());
    }
    Ok(())
}
