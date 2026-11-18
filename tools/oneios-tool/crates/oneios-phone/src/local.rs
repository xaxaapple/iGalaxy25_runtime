//! The local forms of `phone log`, `phone log --list` and `phone cpu`: the same readings made on the phone itself,
//! by `oneios-tool serve` (README "serve": `log`, `log-list`, `cpu`), without adb and without a shell. Read only.
//!
//! The phone's rule holds here as in the adb forms: **named files only** under /sys and /proc (the CPUs 0 to 7 by
//! name, Android's four cpusets, xnuhost's own files by the pid the stage names). Nothing lists a folder of /sys
//! or /proc, and nothing walks one: a root tree walk panicked the phone's kernel on 2026-11-12. The stage's own
//! `logs/` folder is listed (it is ours, on /data). The adb form of `phone cpu` also lists xnuhost's threads, which
//! it asks `ps` for; this form starts no program, so it gives xnuhost's own line (its CPU, nice and thread count
//! from `/proc/PID/stat`) and no per-thread list.

use crate::cpu;
use crate::log::{rel_path_of, Filter, FILES};
use anyhow::{bail, Context, Result};
use oneios_common::job;
use serde_json::json;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

fn read_word(path: &str) -> String {
    std::fs::read_to_string(path).map(|t| t.trim().to_string()).unwrap_or_default()
}

/// The words of a file joined by commas (`tr ' ' ,` in the adb form's script).
fn read_list(path: &str) -> String {
    read_word(path).split_whitespace().collect::<Vec<_>>().join(",")
}

/// What `cpu::cpu_script` prints on the phone, read here from the same named files below `sys_root` ("" on the
/// phone): one `cpu N key value ...` line per CPU that is there, `boost`, the four `cpuset` lines, and `xnuhost`.
pub fn cpu_lines(sys_root: &str, stage: &str) -> String {
    let mut out = String::new();
    for n in 0..8 {
        let d = format!("{sys_root}/sys/devices/system/cpu/cpu{n}");
        let f = format!("{d}/cpufreq");
        if !Path::new(&format!("{f}/cpuinfo_max_freq")).exists() && !Path::new(&format!("{d}/cpu_capacity")).exists() {
            continue;
        }
        let top = read_word(&format!("{f}/scaling_available_frequencies")).split_whitespace().filter_map(|w| w.parse::<u64>().ok()).max().map(|k| k.to_string()).unwrap_or_default();
        out += &format!(
            "cpu {n} midr {} cap {} cluster {} online {} related {} max {} smax {} smin {} cur {} gov {} boostf {} top {top}\n",
            read_word(&format!("{d}/regs/identification/midr_el1")),
            read_word(&format!("{d}/cpu_capacity")),
            read_word(&format!("{d}/topology/cluster_id")),
            read_word(&format!("{d}/online")),
            read_list(&format!("{f}/related_cpus")),
            read_word(&format!("{f}/cpuinfo_max_freq")),
            read_word(&format!("{f}/scaling_max_freq")),
            read_word(&format!("{f}/scaling_min_freq")),
            read_word(&format!("{f}/scaling_cur_freq")),
            read_word(&format!("{f}/scaling_governor")),
            read_list(&format!("{f}/scaling_boost_frequencies")),
        );
    }
    out += &format!("boost {}\n", read_word(&format!("{sys_root}/sys/devices/system/cpu/cpufreq/boost")));
    for c in ["top-app", "foreground", "background", "system-background"] {
        out += &format!("cpuset {c} {}\n", read_word(&format!("{sys_root}/dev/cpuset/{c}/cpus")));
    }
    let pid = read_word(&format!("{stage}/session/xnuhost.pid"));
    let proc = format!("{sys_root}/proc/{pid}");
    let alive = !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) && std::fs::read(format!("{proc}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("xnuhost"));
    if alive {
        let status = std::fs::read_to_string(format!("{proc}/status")).unwrap_or_default();
        let allowed = status.lines().find_map(|l| l.strip_prefix("Cpus_allowed_list:")).map(str::trim).unwrap_or("");
        out += &format!("xnuhost {pid} cpuset {} allowed {allowed}\n", read_word(&format!("{proc}/cpuset")));
        // /proc/PID/stat: after the name in brackets, the 17th word is nice, the 18th the thread count, the 37th the
        // CPU it last ran on
        let stat = std::fs::read_to_string(format!("{proc}/stat")).unwrap_or_default();
        let words: Vec<&str> = stat.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().collect()).unwrap_or_default();
        if let (Some(nice), Some(threads), Some(psr)) = (words.get(16), words.get(17), words.get(36)) {
            out += &format!("process {threads} {psr} {nice}\n");
        }
    } else {
        out += "xnuhost none\n";
    }
    out
}

/// `phone cpu` on the phone itself: the report (the adb form's lines), and a `row` per CPU for `serve`.
pub fn cpu(sys_root: &str, stage: &str) -> Result<usize> {
    let lines = cpu_lines(sys_root, stage);
    let cpus: Vec<cpu::CpuLine> = lines.lines().filter_map(cpu::parse_cpu).collect();
    if cpus.is_empty() {
        bail!("no CPU could be read under {sys_root}/sys/devices/system/cpu (cpu0 to cpu7)");
    }
    for c in &cpus {
        let khz = |key: &str| c.get(key).parse::<u64>().ok();
        let capped = matches!((khz("smax"), khz("max")), (Some(cap), Some(max)) if cap < max);
        job::row(
            "cpu",
            &[
                ("n", json!(c.n)),
                ("core", json!(cpu::core_name(c.get("midr")))),
                ("capacity", json!(c.get("cap").parse::<u64>().ok())),
                ("related", json!(c.get("related"))),
                ("governor", json!(c.get("gov"))),
                ("max_khz", json!(khz("max"))),
                ("scaling_max_khz", json!(khz("smax"))),
                ("cur_khz", json!(khz("cur"))),
                ("capped", json!(capped)),
            ],
        );
    }
    print!("{}", cpu::render(&lines));
    Ok(cpus.len())
}

/// `phone log --list` on the phone itself: each named log with its size and the second of its last change, then
/// the earlier sessions' logs (`logs/xnuhost-EPOCH.log`, newest first). Returns how many are there.
pub fn log_list(stage: &str) -> Result<usize> {
    use std::os::unix::fs::MetadataExt;
    let mut there = 0;
    let mut one = |name: &str, rel: &str, what: &str| {
        match std::fs::metadata(format!("{stage}/{rel}")) {
            Ok(meta) if meta.is_file() => {
                there += 1;
                println!("{name:9} {:>10} bytes  changed {}  {rel}: {what}", meta.len(), meta.mtime());
                job::row("log", &[("name", json!(name)), ("path", json!(rel)), ("bytes", json!(meta.len())), ("changed", json!(meta.mtime()))]);
            }
            _ => {
                println!("{name:9} {:>10}        {rel}: {what}", "missing");
                job::row("log", &[("name", json!(name)), ("path", json!(rel)), ("missing", json!(true))]);
            }
        }
    };
    for (name, rel, what) in FILES {
        one(name, rel, what);
    }
    let mut earlier: Vec<String> = match std::fs::read_dir(format!("{stage}/logs")) {
        Ok(dir) => dir.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| rel_path_of(&format!("logs/{n}")).is_ok()).collect(),
        Err(_) => Vec::new(),
    };
    earlier.sort();
    for name in earlier.iter().rev() {
        let rel = format!("logs/{name}");
        one(&rel, &rel, "an earlier session's log");
    }
    Ok(there)
}

pub struct LogOptions<'a> {
    /// One of `FILES`' names, or `logs/xnuhost-EPOCH.log`.
    pub file: &'a str,
    /// Only the last N lines (before the filters, as the adb form's `tail -n N`).
    pub tail: Option<u64>,
    /// Write the lines the filter keeps to this file instead of printing them.
    pub out: Option<&'a str>,
}

#[derive(Debug, Default, PartialEq)]
pub struct LogRead {
    /// Lines read (after `tail`).
    pub lines: u64,
    /// Lines the filter kept.
    pub matched: u64,
    /// The file's size.
    pub bytes: u64,
}

/// `phone log` on the phone itself: the named file of the stage, read once (no follow), filtered, printed or
/// written to `out`.
pub fn log(stage: &str, o: &LogOptions, mut filter: Filter) -> Result<LogRead> {
    let rel = rel_path_of(o.file)?;
    let path = format!("{stage}/{rel}");
    let file = std::fs::File::open(&path).with_context(|| format!("{path} (log-list shows which logs are there)"))?;
    let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut out: Box<dyn Write> = match o.out {
        Some(p) => Box::new(std::io::BufWriter::new(std::fs::File::create(p).with_context(|| p.to_string())?)),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut read = LogRead { bytes, ..Default::default() };
    let mut take = |line: &str, read: &mut LogRead| -> Result<()> {
        read.lines += 1;
        if filter.takes(line) {
            read.matched += 1;
            writeln!(out, "{line}").context("writing the log's lines")?;
        }
        Ok(())
    };
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut raw = Vec::new();
    // the last N lines: kept as they pass, so the file is read once and only N lines are held
    let mut last: VecDeque<String> = VecDeque::new();
    let mut seen = 0u64;
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).with_context(|| path.clone())? == 0 {
            break;
        }
        seen += 1;
        if seen % 4096 == 0 {
            job::check()?;
        }
        let text = String::from_utf8_lossy(&raw);
        let line = text.trim_end_matches(['\n', '\r']);
        match o.tail {
            Some(n) => {
                if n > 0 {
                    if last.len() as u64 == n {
                        last.pop_front();
                    }
                    last.push_back(line.to_string());
                }
            }
            None => take(line, &mut read)?,
        }
    }
    for line in &last {
        take(line, &mut read)?;
    }
    drop(take);
    out.flush().context("writing the log's lines")?;
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oneios-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel.trim_start_matches('/'));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A stand-in of the S24 Ultra's files: two of its CPUs, the cpusets, and a session.
    fn phone(root: &Path, stage: &Path, session: bool) {
        for (n, midr, cap, related, max, smax) in [(0, "0x00000000410fd801", "325", "0 1", "2265600", "2265600"), (7, "0x00000000411fd821", "1024", "7", "3052800", "2400000")] {
            let d = format!("sys/devices/system/cpu/cpu{n}");
            put(root, &format!("{d}/regs/identification/midr_el1"), &format!("{midr}\n"));
            put(root, &format!("{d}/cpu_capacity"), &format!("{cap}\n"));
            put(root, &format!("{d}/online"), "1\n");
            put(root, &format!("{d}/cpufreq/related_cpus"), &format!("{related}\n"));
            put(root, &format!("{d}/cpufreq/cpuinfo_max_freq"), &format!("{max}\n"));
            put(root, &format!("{d}/cpufreq/scaling_max_freq"), &format!("{smax}\n"));
            put(root, &format!("{d}/cpufreq/scaling_cur_freq"), "1000000\n");
            put(root, &format!("{d}/cpufreq/scaling_governor"), "schedutil\n");
            put(root, &format!("{d}/cpufreq/scaling_available_frequencies"), &format!("480000 {max} 1200000\n"));
        }
        put(root, "sys/devices/system/cpu/cpu7/cpufreq/scaling_boost_frequencies", "3398400\n");
        put(root, "sys/devices/system/cpu/cpufreq/boost", "1\n");
        put(root, "dev/cpuset/top-app/cpus", "0-7\n");
        put(root, "dev/cpuset/background/cpus", "0-1\n");
        if session {
            put(stage, "session/xnuhost.pid", "4242\n");
            put(root, "proc/4242/cmdline", "/data/local/tmp/oneios/runtime/bin/xnuhost\0--conf\0x\0");
            put(root, "proc/4242/cpuset", "/top-app\n");
            put(root, "proc/4242/status", "Name:\txnuhost\nCpus_allowed_list:\t0-7\n");
            put(root, "proc/4242/stat", "4242 (xnu host) S 1 4242 4242 0 -1 4194560 100 0 0 0 50 60 0 0 20 0 23 0 100 1000 200 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 6 0 0 0 0 0\n");
        }
    }

    #[test]
    fn cpu_reads_the_named_files_as_the_adb_script_does() {
        let (root, stage) = (tmp("cpu-root"), tmp("cpu-stage"));
        phone(&root, &stage, true);
        let lines = cpu_lines(root.to_str().unwrap(), stage.to_str().unwrap());
        // the lines the adb form's script prints (cpu.rs parses and renders both)
        assert!(lines.contains("cpu 7 midr 0x00000000411fd821 cap 1024 cluster  online 1 related 7 max 3052800 smax 2400000 smin  cur 1000000 gov schedutil boostf 3398400 top 3052800\n"), "{lines}");
        assert!(lines.contains("cpu 0 midr 0x00000000410fd801 cap 325 cluster  online 1 related 0,1 max 2265600"), "{lines}");
        assert!(!lines.contains("cpu 3 "), "a CPU that is not there has no line");
        assert!(lines.contains("boost 1\n") && lines.contains("cpuset top-app 0-7\n") && lines.contains("cpuset foreground \n"));
        assert!(lines.contains("xnuhost 4242 cpuset /top-app allowed 0-7\n") && lines.contains("process 23 6 0\n"), "{lines}");
        let text = cpu::render(&lines);
        assert!(text.contains("cpu7 Cortex-X4") && text.contains("CAPPED: scaling_max_freq 2400.0 MHz") && text.contains("boost 3398400"), "{text}");
        assert!(text.contains("cpuset foreground         cpus ?") && text.contains("xnuhost 4242 cpuset /top-app allowed 0-7"), "{text}");
        assert!(text.contains("23 threads") && text.contains("cpu6") && text.contains("nice 0"), "{text}");
        assert_eq!(cpu(root.to_str().unwrap(), stage.to_str().unwrap()).unwrap(), 2);

        for d in [&root, &stage] {
            let _ = std::fs::remove_dir_all(d);
        }

        // no session: a pid file of a process that is gone, or none
        let (root, stage) = (tmp("cpu-root2"), tmp("cpu-stage2"));
        phone(&root, &stage, false);
        put(&stage, "session/xnuhost.pid", "999\n");
        let lines = cpu_lines(root.to_str().unwrap(), stage.to_str().unwrap());
        assert!(lines.ends_with("xnuhost none\n") && cpu::render(&lines).contains("xnuhost: no session"));
        // a PC: nothing to read is said, not printed as an empty report
        let empty = tmp("cpu-empty");
        assert!(cpu(empty.to_str().unwrap(), stage.to_str().unwrap()).unwrap_err().to_string().contains("no CPU could be read"));
        for d in [root, stage, empty] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_local_forms_name_their_files() {
        // the source of the two readings names /sys and /proc files one by one: no folder of them is listed
        let source = include_str!("local.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for line in code.lines().filter(|l| l.contains("read_dir") || l.contains("WalkDir") || l.contains("glob")) {
            assert!(line.contains("{stage}/logs"), "only the stage's own logs/ folder is listed: {line}");
        }
    }

    #[test]
    fn log_reads_filters_and_tails() {
        let stage = tmp("log");
        let text: String = (0..10).map(|i| format!("[{:>9}.000] guest     {}\n", i, if i % 2 == 0 { "oneios-gpu-report: frame" } else { "oneios-hid: touch" })).collect();
        put(&stage, "session/xnuhost.log", &text);
        put(&stage, "logs/xnuhost-1794000000.log", "old\n");
        put(&stage, "logs/not-a-log.txt", "x\n");
        let s = stage.to_str().unwrap();
        let out = stage.join("out.txt");
        let all = |tail, filter: Filter| log(s, &LogOptions { file: "xnuhost", tail, out: Some(out.to_str().unwrap()) }, filter).unwrap();
        assert_eq!(all(None, Filter::new(&[], &[], false, None).unwrap()), LogRead { lines: 10, matched: 10, bytes: text.len() as u64 });
        assert_eq!(std::fs::read_to_string(&out).unwrap(), text);
        // a named filter, and --since
        let read = all(None, Filter::new(&[], &["gpu"], false, Some(5.0)).unwrap());
        assert_eq!((read.lines, read.matched), (10, 2));
        assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 2);
        // the last 3 lines, then the filter: lines 7, 8, 9, of which 8 is the gpu's
        let read = all(Some(3), Filter::new(&[], &["gpu"], false, None).unwrap());
        assert_eq!((read.lines, read.matched), (3, 1));
        assert!(std::fs::read_to_string(&out).unwrap().starts_with("[        8.000]"));
        assert_eq!(all(Some(0), Filter::new(&[], &[], false, None).unwrap()).lines, 0);
        // an earlier session's log by its name; nothing else of the stage can be named
        assert_eq!(log(s, &LogOptions { file: "logs/xnuhost-1794000000.log", tail: None, out: Some(out.to_str().unwrap()) }, Filter::new(&[], &[], false, None).unwrap()).unwrap().lines, 1);
        for bad in ["logs/../session/xnuhost.pid", "session/xnuhost.pid", "/etc/passwd", "logs/not-a-log.txt"] {
            assert!(log(s, &LogOptions { file: bad, tail: None, out: None }, Filter::new(&[], &[], false, None).unwrap()).is_err(), "{bad}");
        }
        // a log that is not there says where to look
        assert!(log(s, &LogOptions { file: "switch", tail: None, out: None }, Filter::new(&[], &[], false, None).unwrap()).unwrap_err().to_string().contains("log-list"));
        // the list: the session's log and the earlier one are there, the others missing
        assert_eq!(log_list(s).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&stage);
    }
}
