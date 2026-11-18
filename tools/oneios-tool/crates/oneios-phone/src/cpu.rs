//! `oneios-tool phone cpu [--trace SECONDS] [--out FILE]`: where iOS's CPU work can run and how fast, read only
//! (host-overhead, 2026-11-15). With or without a session:
//! - each CPU's core (MIDR part), capacity, frequency domain (`related_cpus`), `cpuinfo_max_freq`, `scaling_max_freq`
//!   (a cap someone wrote), `scaling_governor`, the boost frequencies and the current clock; `cpufreq/boost`;
//! - the CPUs of Android's cpusets (top-app, foreground, background, system-background);
//! - in a session, xnuhost's threads (TID, the CPU it last ran on, nice, cpuset, uclamp) and the iOS threads of its task
//!   processes, counted by the CPU they last ran on and their uclamp.min.
//! `--trace SECONDS` also records a Perfetto trace (sched, freq, idle) of that long and copies it to `--out`
//! (default `oneios-cpu.pftrace`).
//!
//! Named files only (the phone's rule): the CPUs 0-7 by name, each thread's files by the TID `ps -T` gives; no glob and no
//! directory walk under /sys or /proc. Nothing is written on the phone but the trace file under
//! /data/misc/perfetto-traces, which is removed after the copy.
use crate::adb::{Adb, STAGE};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::copy;

/// The trace on the phone while it is recorded.
pub const TRACE_FILE: &str = "/data/misc/perfetto-traces/oneios-cpu.pftrace";

/// The read: one line per item, `cpu N key value ...`, `cpuset NAME CPUS`, `thread TID PSR NI NAME | cpuset | uclamp`,
/// `ios PSR MIN` per iOS thread.
pub fn cpu_script() -> String {
    let mut s = String::new();
    s += "for n in 0 1 2 3 4 5 6 7; do\n";
    s += "  d=/sys/devices/system/cpu/cpu$n\n";
    s += "  [ -e $d/cpufreq/cpuinfo_max_freq ] || [ -e $d/cpu_capacity ] || continue\n";
    s += "  f=$d/cpufreq\n";
    s += "  echo \"cpu $n midr $(cat $d/regs/identification/midr_el1 2>/dev/null) cap $(cat $d/cpu_capacity 2>/dev/null) \
cluster $(cat $d/topology/cluster_id 2>/dev/null) online $(cat $d/online 2>/dev/null) related $(cat $f/related_cpus 2>/dev/null | tr ' ' ,) \
max $(cat $f/cpuinfo_max_freq 2>/dev/null) smax $(cat $f/scaling_max_freq 2>/dev/null) smin $(cat $f/scaling_min_freq 2>/dev/null) \
cur $(cat $f/scaling_cur_freq 2>/dev/null) gov $(cat $f/scaling_governor 2>/dev/null) boostf $(cat $f/scaling_boost_frequencies 2>/dev/null | tr ' ' ,) \
top $(cat $f/scaling_available_frequencies 2>/dev/null | tr ' ' '\\n' | sort -n | tail -1)\"\n";
    s += "done\n";
    s += "echo \"boost $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)\"\n";
    s += "for c in top-app foreground background system-background; do echo \"cpuset $c $(cat /dev/cpuset/$c/cpus 2>/dev/null)\"; done\n";
    s += &format!("p=$(cat {STAGE}/session/xnuhost.pid 2>/dev/null)\n");
    s += "if [ -n \"$p\" ] && [ -e /proc/$p/cmdline ] && tr '\\0' ' ' < /proc/$p/cmdline | grep -q xnuhost; then\n";
    s += "  echo \"xnuhost $p cpuset $(cat /proc/$p/cpuset 2>/dev/null) allowed $(grep Cpus_allowed_list /proc/$p/status | awk '{print $2}')\"\n";
    s += "  ps -T -p $p -o TID=,PSR=,NI=,NAME= 2>/dev/null | while read t c n name; do\n";
    s += "    u=$(grep -E '^uclamp\\.(min|max)' /proc/$p/task/$t/sched 2>/dev/null | awk '{print $3}' | tr '\\n' /)\n";
    s += "    echo \"thread $t $c $n $name $(cat /proc/$p/task/$t/cpuset 2>/dev/null) ${u:-?}\"\n";
    s += "  done\n";
    // the task factory (a child of xnuhost named xnuhost-tasks) and its children, the iOS task processes
    s += "  fac=$(ps -o PID=,NAME= --ppid $p 2>/dev/null | awk '$2 == \"xnuhost-tasks\" { print $1; exit }')\n";
    s += "  [ -n \"$fac\" ] && ps -T -o PID=,TID=,PSR= --ppid $fac 2>/dev/null | while read pid t c; do\n";
    s += "    echo \"ios $c $(grep -E '^uclamp\\.min' /proc/$pid/task/$t/sched 2>/dev/null | awk '{print $3}')\"\n";
    s += "  done\n";
    s += "else echo \"xnuhost none\"; fi\n";
    s
}

#[derive(Debug, Default, PartialEq)]
pub struct CpuLine {
    pub n: u32,
    pub fields: Vec<(String, String)>,
}

impl CpuLine {
    pub fn get(&self, key: &str) -> &str {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("")
    }
}

/// The core's name from `midr_el1` (0x...: the part number in bits 4-15).
pub fn core_name(midr: &str) -> String {
    let v = u64::from_str_radix(midr.trim_start_matches("0x"), 16).unwrap_or(0);
    match (v >> 4) & 0xfff {
        0xd80 => "Cortex-A520".into(),
        0xd81 => "Cortex-A720".into(),
        0xd82 => "Cortex-X4".into(),
        0xd46 => "Cortex-A510".into(),
        0xd4d => "Cortex-A715".into(),
        0xd4e => "Cortex-X3".into(),
        0 => "?".into(),
        p => format!("part 0x{p:x}"),
    }
}

/// `cpu N key value key value ...` (a missing value is an empty word, kept as "").
pub fn parse_cpu(line: &str) -> Option<CpuLine> {
    let rest = line.strip_prefix("cpu ")?;
    let mut words = rest.split(' ');
    let n = words.next()?.parse().ok()?;
    let words: Vec<&str> = words.collect();
    let keys = ["midr", "cap", "cluster", "online", "related", "max", "smax", "smin", "cur", "gov", "boostf", "top"];
    let mut fields = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if keys.contains(&words[i]) {
            let value = if i + 1 < words.len() && !keys.contains(&words[i + 1]) { words[i + 1] } else { "" };
            fields.push((words[i].to_string(), value.to_string()));
            i += if value.is_empty() { 1 } else { 2 };
        } else {
            i += 1;
        }
    }
    Some(CpuLine { n, fields })
}

fn mhz(khz: &str) -> String {
    khz.parse::<u64>().map(|k| format!("{}.{} MHz", k / 1000, k % 1000 / 100)).unwrap_or_else(|_| "?".into())
}

/// The report, from the script's output.
pub fn render(out: &str) -> String {
    let mut text = String::new();
    let mut ios: std::collections::BTreeMap<(String, String), u32> = Default::default();
    for line in out.lines() {
        if let Some(c) = parse_cpu(line) {
            let cap = c.get("smax");
            let capped = !cap.is_empty() && cap != c.get("max") && cap.parse::<u64>().ok() < c.get("max").parse::<u64>().ok();
            text += &format!(
                "cpu{} {:<12} capacity {:>4}  domain cpus {:<6} governor {:<10} top {} (cpuinfo_max_freq; table top {}){}{}  now {}\n",
                c.n,
                core_name(c.get("midr")),
                c.get("cap"),
                c.get("related"),
                c.get("gov"),
                mhz(c.get("max")),
                mhz(c.get("top")),
                if c.get("boostf").is_empty() { String::new() } else { format!(", boost {}", c.get("boostf")) },
                if capped { format!("  CAPPED: scaling_max_freq {}", mhz(cap)) } else { String::new() },
                mhz(c.get("cur")),
            );
        } else if let Some(v) = line.strip_prefix("boost ") {
            text += &format!("cpufreq boost: {}\n", if v.is_empty() { "(no switch)" } else { v });
        } else if let Some(v) = line.strip_prefix("cpuset ") {
            let (name, cpus) = v.split_once(' ').unwrap_or((v, ""));
            text += &format!("cpuset {name:<18} cpus {}\n", if cpus.is_empty() { "?" } else { cpus });
        } else if let Some(v) = line.strip_prefix("xnuhost ") {
            text += &if v == "none" { "xnuhost: no session\n".to_string() } else { format!("xnuhost {v}\n") };
        } else if let Some(v) = line.strip_prefix("process ") {
            // the local form (local.rs): xnuhost's own line of /proc/PID/stat, where the adb form lists the threads
            let w: Vec<&str> = v.split_whitespace().collect();
            if w.len() == 3 {
                text += &format!("  xnuhost itself: {} threads, last on cpu{}, nice {} (thread by thread: `oneios-tool phone cpu` over adb)\n", w[0], w[1], w[2]);
            }
        } else if let Some(v) = line.strip_prefix("thread ") {
            let w: Vec<&str> = v.split_whitespace().collect();
            if w.len() >= 4 {
                text += &format!("  thread {:<7} on cpu{:<2} nice {:>3}  {:<16} cpuset {:<20} uclamp min/max {}\n", w[0], w[1], w[2], w[3],
                                 w.get(4).unwrap_or(&"?"), w.get(5).unwrap_or(&"?"));
            }
        } else if let Some(v) = line.strip_prefix("ios ") {
            let (psr, min) = v.split_once(' ').unwrap_or((v, ""));
            *ios.entry((psr.to_string(), if min.is_empty() { "?".into() } else { min.to_string() })).or_default() += 1;
        }
    }
    if !ios.is_empty() {
        text += "iOS threads (in the task processes), by the CPU they last ran on and their uclamp.min:\n";
        for ((psr, min), n) in &ios {
            text += &format!("  cpu{psr:<2} uclamp.min {min:>4}: {n} threads\n");
        }
    }
    text
}

/// `phone cpu`.
pub fn cpu(adb: &Adb, trace: Option<u64>, out: Option<&str>) -> Result<()> {
    adb.need_root()?;
    let r = adb.root(&cpu_script())?;
    if !r.ok() && r.out.is_empty() {
        bail!("the read failed on the phone (status {}): {}", r.status, r.err.trim());
    }
    print!("{}", render(&r.out));
    if let Some(seconds) = trace {
        let local = out.unwrap_or("oneios-cpu.pftrace");
        println!("trace: {seconds} s of sched, freq and idle (perfetto) ...");
        adb.root_ok(&format!("perfetto -o {TRACE_FILE} -t {seconds}s sched freq idle >/dev/null 2>&1; ls -l {TRACE_FILE}\n"), "perfetto")?;
        let mut child = adb.exec_out(&format!("su -c 'cat {TRACE_FILE}'"))?;
        let mut file = File::create(local).with_context(|| format!("cannot write {local}"))?;
        let n = copy(child.stdout.as_mut().context("no output from adb")?, &mut file)?;
        child.wait()?;
        adb.root(&format!("rm -f {TRACE_FILE}\n"))?;
        println!("trace: {n} bytes in {local} (open it in ui.perfetto.dev)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_reads_named_files_only() {
        let s = cpu_script();
        assert!(!s.contains('*'), "no glob");
        assert!(!s.contains("ls /proc") && !s.contains("ls /sys") && !s.contains("find "), "no directory walk");
        assert!(s.contains("/sys/devices/system/cpu/cpu$n") && s.contains("for n in 0 1 2 3 4 5 6 7"));
        assert!(s.contains("/proc/$p/task/$t/sched") && s.contains("ps -T -p $p"));
    }

    #[test]
    fn a_cpu_line_and_its_report() {
        let line = "cpu 7 midr 0x00000000411fd821 cap 1024 cluster 0 online 1 related 7 max 3052800 smax 2400000 smin 480000 cur 2400000 gov schedutil boostf 3398400 top 3052800";
        let c = parse_cpu(line).unwrap();
        assert_eq!(c.n, 7);
        assert_eq!(c.get("max"), "3052800");
        assert_eq!(c.get("boostf"), "3398400");
        assert_eq!(core_name(c.get("midr")), "Cortex-X4");
        let text = render(&format!("{line}\nboost 1\ncpuset top-app 0-7\ncpuset background 0-1\nxnuhost 1234 cpuset /top-app allowed 0-7\nthread 1240 7 0 xnu-cpu7 /top-app 1024/1024/\nios 7 1024\nios 7 1024\nios 0 0\n"));
        assert!(text.contains("cpu7 Cortex-X4") && text.contains("CAPPED: scaling_max_freq 2400.0 MHz") && text.contains("boost 3398400"));
        assert!(text.contains("cpuset top-app") && text.contains("thread 1240") && text.contains("xnu-cpu7"));
        assert!(text.contains("cpu7  uclamp.min 1024: 2 threads") && text.contains("cpu0  uclamp.min    0: 1 threads"));
        // an empty value is kept as empty, not as the next key
        let c = parse_cpu("cpu 0 midr 0x410fd801 cap 325 cluster online 1 related 0,1 max 2265600 smax 2265600 smin cur gov schedutil boostf top 2265600").unwrap();
        assert_eq!(c.get("cluster"), "");
        assert_eq!(c.get("online"), "1");
        assert_eq!(c.get("boostf"), "");
        assert!(!render("cpu 0 midr 0x410fd801 cap 325 related 0,1 max 2265600 smax 2265600 gov schedutil top 2265600").contains("CAPPED"));
    }
}
