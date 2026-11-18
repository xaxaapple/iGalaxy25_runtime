//! Tests of `ioslog health` on text written by hand in the form `ioslog show` writes (tests/ioslog/health.txt and
//! the lines here): no Apple file. The counts of the fixture are written out here; a new kind adds its lines to
//! the fixture and its numbers to `the_fixture`.

use super::*;

const FIXTURE: &str = include_str!("../../../tests/ioslog/health.txt");

fn report(text: &str) -> Report {
    let mut counter = Counter::new(2).unwrap();
    count_text(text.as_bytes(), &mut counter).unwrap();
    counter.finish("health.txt".into(), "the entries of a text file (ioslog show's)".into())
}

fn kind<'a>(report: &'a Report, name: &str) -> &'a Count {
    report.kinds.iter().find(|c| c.kind.name == name).unwrap_or_else(|| panic!("no kind {name}"))
}

/// `name count` of a list.
fn counts(list: &[(String, Tally)]) -> Vec<(&str, u64)> {
    list.iter().map(|(name, tally)| (name.as_str(), tally.count)).collect()
}

#[test]
fn the_table() {
    // every text and name compiles, and the names are one word each and differ
    Counter::new(0).unwrap();
    let names: Vec<&str> = KINDS.iter().map(|k| k.name).collect();
    assert_eq!(names, ["hangs", "watchdog", "xpc-timeouts", "jetsam", "crashes", "stuck-idle", "encode-fail"]);
    for kind in KINDS {
        assert!(!kind.texts.is_empty() && !kind.what.is_empty() && !kind.name.contains(' '), "{}", kind.name);
        // a kind filed by the name in its lines needs a way to find it; one filed by the logger that also names
        // something says what the names are
        assert!(kind.by == By::Logger || !kind.who.is_empty(), "{}", kind.name);
        assert!(kind.who.is_empty() || kind.by == By::Named || !kind.names.is_empty(), "{}", kind.name);
    }
}

#[test]
fn the_fixture() {
    let r = report(FIXTURE);
    assert_eq!(r.entries, 36);
    assert_eq!(r.head(), "health.txt: the entries of a text file (ioslog show's), 36 entries, from 2026-11-15 09:00:00.000000Z to 2026-11-15 09:10:00.000000Z (600 s)");

    // hangs: by the process that hung, with the seconds
    let hangs = kind(&r, "hangs");
    assert_eq!(hangs.all.count, 8);
    assert!((hangs.all.seconds - 109.73).abs() < 1e-9 && hangs.all.longest == 61.27 && hangs.all.over == 5);
    assert_eq!(hangs.longest_by, "Preferences");
    assert_eq!(counts(&hangs.loggers), [("Preferences", 4), ("SpringBoard", 2), ("MobileSafari", 2)]);
    let preferences = hangs.loggers[0].1;
    // 1.00 s is not over 1 s
    assert!((preferences.seconds - 64.89).abs() < 1e-9 && preferences.longest == 61.27 && preferences.over == 2);
    let springboard = hangs.loggers[1].1;
    assert!((springboard.seconds - 40.09).abs() < 1e-9 && springboard.longest == 39.34 && springboard.over == 1);
    // `3.5 s` with a space, and a hang in the second line of a message
    let safari = hangs.loggers[2].1;
    assert!((safari.seconds - 4.75).abs() < 1e-9 && safari.longest == 3.5 && safari.over == 2);
    assert!(hangs.names.is_empty());

    // watchdog: by the logger; the names are the extensions (the line's own, or RunningBoard's name of the process)
    let watchdog = kind(&r, "watchdog");
    assert_eq!(watchdog.all.count, 5);
    assert_eq!(watchdog.texts, [1, 3, 1]);
    assert_eq!(counts(&watchdog.loggers), [("chronod", 3), ("SpringBoard", 1), ("backboardd", 1)]);
    assert_eq!(counts(&watchdog.names), [("com.example.clock.widget", 2), ("com.example.weather.widget", 2)]);

    // XPC timeouts: `Default XPC timeout` also holds `XPC timeout` and is counted once, under the first text
    let xpc = kind(&r, "xpc-timeouts");
    assert_eq!(xpc.all.count, 4);
    assert_eq!(xpc.texts, [2, 1, 1]);
    assert_eq!(counts(&xpc.loggers), [("SpringBoard", 3), ("Preferences", 1)]);
    assert_eq!(counts(&xpc.names), [("com.example.photos.service", 2), ("com.example.settings.service", 1)]);

    // the kernel's lines: by the process they name
    let jetsam = kind(&r, "jetsam");
    assert_eq!((jetsam.all.count, jetsam.texts.as_slice()), (4, &[1u64, 3][..]));
    assert_eq!(counts(&jetsam.names), [("MobileSafari", 2), ("Preferences", 1), ("nfcd", 1)]);
    assert_eq!(counts(&jetsam.loggers), [("kernel", 4)]);
    let crashes = kind(&r, "crashes");
    assert_eq!((crashes.all.count, crashes.texts.as_slice()), (3, &[2u64, 1][..]));
    assert_eq!(counts(&crashes.names), [("chronod", 2), ("Preferences", 1)]);
    let stuck = kind(&r, "stuck-idle");
    assert_eq!(counts(&stuck.names), [("profiled", 2), ("coreauthd", 1)]);

    let encode = kind(&r, "encode-fail");
    assert_eq!(counts(&encode.loggers), [("chronod", 2), ("SpringBoard", 1)]);
    assert_eq!(counts(&encode.names), [("kCMBaseObjectError_UnsupportedOperation [-12782]", 2), ("-16993", 1)]);

    // the first and last line of a kind, and its examples (two were asked for)
    assert_eq!((time_text(hangs.first), time_text(hangs.last)), ("2026-11-15 09:00:10.000000Z".to_string(), "2026-11-15 09:01:26.000000Z".to_string()));
    assert_eq!(hangs.examples.len(), 2);
    assert_eq!(hangs.examples[0], "2026-11-15 09:00:10.000000Z Preferences[512] Default [com.apple.hangtracer:] Hang detected: 0.52s (always-on hang reporting)");
}

#[test]
fn the_lines() {
    let r = report(FIXTURE);
    assert_eq!(
        r.summary(2),
        [
            "hangs              8  109.7 s in all, longest 61.27 s (Preferences), 5 over 1 s; Preferences 4 (64.9 s, longest 61.27 s, 2 over 1 s), SpringBoard 2 (40.1 s, longest 39.34 s, 1 over 1 s), and 1 more with 2",
            "watchdog           5  chronod 3, SpringBoard 1, and 1 more with 1; named: com.example.clock.widget 2, com.example.weather.widget 2",
            "xpc-timeouts       4  SpringBoard 3, Preferences 1; services: com.example.photos.service 2, com.example.settings.service 1",
            "jetsam             4  MobileSafari 2, Preferences 1, and 1 more with 1",
            "crashes            3  chronod 2, Preferences 1",
            "stuck-idle         3  profiled 2, coreauthd 1",
            "encode-fail        3  chronod 2, SpringBoard 1; errors: kCMBaseObjectError_UnsupportedOperation [-12782] 2, -16993 1",
        ]
    );
    // the report: the head, a line per kind; with the details the indented lines below each
    let plain = r.text(TOP, false);
    assert_eq!(plain.lines().count(), 1 + KINDS.len());
    let detail = r.text(TOP, true);
    assert_eq!(summary_of(&detail), r.summary(TOP));
    assert_eq!(summary_of(&plain), r.summary(TOP));
    for line in [
        "    counts: HangTracer's `Hang detected: N s`: a main thread that did not answer for N s, by the process that hung",
        "    first 2026-11-15 09:00:10.000000Z, last 2026-11-15 09:01:26.000000Z",
        "    texts: exit status 1 (SpringBoard 1), extension 3 (chronod 3), fired 1 (backboardd 1)",
        "    by process: chronod 3, SpringBoard 1, backboardd 1",
        "    by process: MobileSafari 2, Preferences 1, nfcd 1",
        "    logged by: kernel 4",
        "    e.g. 2026-11-15 09:05:00.000000Z kernel[0] Default chronod[87] Corpse allowed 1 of 5",
    ] {
        assert!(detail.lines().any(|l| l == line), "no line {line:?} in\n{detail}");
    }
}

#[test]
fn nothing_found() {
    let r = report("2026-11-15 09:00:00.000000Z SpringBoard[30] Default [com.apple.SpringBoard:Common] a quiet boot\n");
    assert_eq!(r.entries, 1);
    assert_eq!(r.summary(TOP), ["hangs              0", "watchdog           0", "xpc-timeouts       0", "jetsam             0", "crashes            0", "stuck-idle         0", "encode-fail        0"]);
    assert!(r.text(TOP, true).contains("hangs              0\n    counts: HangTracer's"));
    // no entry at all: the head says so, and has no times
    let r = report("not a line of ioslog show\n");
    assert_eq!(r.head(), "health.txt: the entries of a text file (ioslog show's), 0 entries");
}

#[test]
fn the_text_form() {
    // what `show` writes for an entry: the scope is not part of the message, RunningBoard's `[kind<name>:pid]` is
    let mut seen = Vec::new();
    let head = Regex::new(TEXT_HEAD).unwrap();
    let scope = Regex::new(TEXT_SCOPE).unwrap();
    for line in [
        "2026-11-15 09:02:11.000000Z SpringBoard[30] Default [com.apple.FrontBoard:Process] [osservice<com.apple.chronod>:87] Process exited",
        "2026-11-15 09:02:11.000000Z runningboardd[29] Default [osservice<com.apple.chronod>:87] Process exited",
        "2026-11-15 09:02:11.5Z wifid[40] Error -[WiFiWatchdogMonitor dealloc]: cleaning up.",
        "2026-11-15 09:02:12Z Google Chrome Helper[7] Info [:only a category] a process name with spaces",
        "2026-11-15 09:02:13.000000Z launchd[1] Simpledump",
    ] {
        let c = head.captures(line).unwrap_or_else(|| panic!("{line}"));
        seen.push((c[2].to_string(), scope.replace(c.get(3).map(|m| m.as_str()).unwrap_or_default(), "").into_owned()));
    }
    let pairs: Vec<(&str, &str)> = seen.iter().map(|(p, m)| (p.as_str(), m.as_str())).collect();
    assert_eq!(
        pairs,
        [
            ("SpringBoard", "[osservice<com.apple.chronod>:87] Process exited"),
            ("runningboardd", "[osservice<com.apple.chronod>:87] Process exited"),
            ("wifid", "-[WiFiWatchdogMonitor dealloc]: cleaning up."),
            ("Google Chrome Helper", "a process name with spaces"),
            ("launchd", ""),
        ]
    );
    assert!(!head.is_match("its second line: Hang detected: 1.25s") && !head.is_match("2026-11-15 09:02:13 launchd[1] Default no Z"));
    // the subsystem of an entry is not taken for the name in its line
    let r = report("2026-11-15 09:03:12.000000Z Preferences[512] Error [com.apple.example:client] XPC timeout\n");
    assert!(kind(&r, "xpc-timeouts").all.count == 1 && kind(&r, "xpc-timeouts").names.is_empty());
}

#[test]
fn the_sources() {
    let dir = std::env::temp_dir().join(format!("oneios-ioslog-health-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("stage/data")).unwrap();
    let text = dir.join("log.txt");
    std::fs::write(&text, FIXTURE).unwrap();
    let text = text.to_str().unwrap();
    assert!(is_text(text) && !is_text(dir.to_str().unwrap()) && !is_text("/nonexistent/log.txt"));
    // a text file: every entry, whatever `last` or `all`; a boot by its UUID cannot be chosen
    let r = read(text, &How::default(), "last", 0).unwrap();
    assert_eq!((r.entries, kind(&r, "hangs").all.count), (36, 8));
    assert!(read(text, &How::default(), "all", 0).is_ok());
    let e = read(text, &How::default(), "9A6A", 0).err().unwrap().to_string();
    assert!(e.ends_with("a text file names no boots; choose the boot when the file is written (ioslog show --boot 9A6A)"), "{e}");
    // a file that is neither is the image reader's to refuse
    let other = dir.join("other.img");
    std::fs::write(&other, b"first block").unwrap();
    assert!(!is_text(other.to_str().unwrap()) && read(other.to_str().unwrap(), &How::default(), "last", 0).is_err());
    // the file `cycle` and `pull-data` leave, and the lines they print
    let out = dir.join("ioslog-health.txt");
    let lines = write(text, &out).unwrap();
    assert_eq!(lines.len(), 1 + KINDS.len());
    assert!(lines[0].ends_with("36 entries, from 2026-11-15 09:00:00.000000Z to 2026-11-15 09:10:00.000000Z (600 s)") && lines[1].starts_with("hangs              8  109.7 s in all"));
    let saved = std::fs::read_to_string(&out).unwrap();
    assert_eq!(summary_of(&saved), lines[1..]);
    assert!(saved.contains("\n    texts: initial call 2 (SpringBoard 2), default 1 (SpringBoard 1), timeout 1 (Preferences 1)\n"));
    // without SOURCE: the stage's own store
    let stage = dir.join("stage");
    let e = staged(stage.to_str().unwrap()).err().unwrap().to_string();
    assert!(e.starts_with(&format!("no SOURCE, and no staged Data store at {}/data/data.img", stage.display())), "{e}");
    std::fs::write(stage.join("data/data.img"), FIXTURE).unwrap();
    assert_eq!(staged(stage.to_str().unwrap()).unwrap(), format!("{}/data/data.img", stage.display()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn command_line() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }
    let parse = |args: &[&str]| Cli::try_parse_from(std::iter::once("health").chain(args.iter().copied()));
    let plain = parse(&[]).unwrap().args;
    assert!(plain.source.is_none() && plain.boot == "last" && plain.top == TOP && plain.examples == EXAMPLES && !plain.detail && plain.stage == "/data/local/tmp/oneios");
    let all = parse(&["data.img", "--boot", "all", "--top", "10", "--detail", "--examples", "0", "-o", "h.txt", "--volume", "Data", "--stage", "/tmp/s"]).unwrap().args;
    assert!(all.source.as_deref() == Some("data.img") && all.boot == "all" && all.top == 10 && all.detail && all.examples == 0 && all.output.is_some());
    assert!(parse(&["a", "b"]).is_err() && parse(&["--top", "x"]).is_err());
}
