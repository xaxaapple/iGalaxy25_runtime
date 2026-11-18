//! `oneios-tool ioslog health` as a command: its lines on stdout, the file of `--output`, the form without SOURCE
//! (the stage's own data/data.img, as on the phone) and its exit statuses. The log is tests/ioslog/health.txt,
//! written by hand in the form `ioslog show` writes (no Apple file); the counts themselves are checked in
//! crates/oneios-ioslog/src/health_tests.rs.
//!
//!   ONEIOS_TOOL=PATH   the binary to run (default: this build's)

use std::path::PathBuf;
use std::process::Command;

fn tool() -> PathBuf {
    std::env::var_os("ONEIOS_TOOL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_oneios-tool")))
}

/// `oneios-tool ioslog health ARGS`: (exit status, stdout, stderr).
fn health(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(tool()).args(["ioslog", "health"]).args(args).output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

const KINDS: [&str; 7] = ["hangs", "watchdog", "xpc-timeouts", "jetsam", "crashes", "stuck-idle", "encode-fail"];

#[test]
fn the_command() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/ioslog/health.txt");
    let work = std::env::temp_dir().join(format!("oneios-tool-health-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("stage/data")).unwrap();

    // a text file: the head, then one line per kind, in the table's order
    let (status, out, err) = health(&[fixture]);
    assert_eq!((status, err.as_str()), (0, ""), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1 + KINDS.len(), "{out}");
    assert!(lines[0].ends_with(": the entries of a text file (ioslog show's), 36 entries, from 2026-11-15 09:00:00.000000Z to 2026-11-15 09:10:00.000000Z (600 s)"), "{out}");
    for (line, kind) in lines[1..].iter().zip(KINDS) {
        assert!(line.starts_with(kind) && !line.starts_with(' '), "{line}");
    }
    assert_eq!(
        lines[1],
        "hangs              8  109.7 s in all, longest 61.27 s (Preferences), 5 over 1 s; Preferences 4 (64.9 s, longest 61.27 s, 2 over 1 s), SpringBoard 2 (40.1 s, longest 39.34 s, 1 over 1 s), MobileSafari 2 (4.8 s, longest 3.50 s, 2 over 1 s)"
    );
    assert_eq!(lines[2], "watchdog           5  chronod 3, SpringBoard 1, backboardd 1; named: com.example.clock.widget 2, com.example.weather.widget 2");

    // --detail and --output: the same lines, and under each kind the indented ones; stdout without --detail has none
    let report = work.join("health.txt");
    let (status, plain, err) = health(&[fixture, "--top", "1", "-o", report.to_str().unwrap()]);
    assert!(status == 0 && err.trim_end() == format!("ioslog: the report with its details is in {}", report.display()), "{err}");
    assert!(plain.lines().all(|l| !l.starts_with(' ')) && plain.contains("\nstuck-idle         3  profiled 2, and 1 more with 1\n"), "{plain}");
    let saved = std::fs::read_to_string(&report).unwrap();
    let (status, detail, _) = health(&[fixture, "--top", "1", "--detail"]);
    assert_eq!((status, detail.as_str()), (0, saved.as_str()));
    let unindented: Vec<&str> = saved.lines().filter(|l| !l.starts_with(' ')).collect();
    assert_eq!(unindented, plain.lines().collect::<Vec<_>>());
    assert!(saved.contains("\n    texts: exit status 1 (SpringBoard 1), extension 3 (chronod 3), fired 1 (backboardd 1)\n") && saved.contains("\n    logged by: kernel 4\n"), "{saved}");
    assert_eq!(saved.lines().filter(|l| l.starts_with("    e.g. ")).count(), 3 * KINDS.len());
    let (_, none, _) = health(&[fixture, "--detail", "--examples", "0"]);
    assert!(!none.contains("e.g.") && none.contains("\n    by process: profiled 2, coreauthd 1\n"), "{none}");

    // without SOURCE: the stage's data/data.img
    let stage = work.join("stage");
    let (status, out, err) = health(&["--stage", stage.to_str().unwrap()]);
    assert!(status == 1 && out.is_empty() && err.starts_with(&format!("ioslog: no SOURCE, and no staged Data store at {}/data/data.img", stage.display())), "{status} {err}");
    std::fs::copy(fixture, stage.join("data/data.img")).unwrap();
    let (status, out, err) = health(&["--stage", stage.to_str().unwrap()]);
    assert!(status == 0 && err.is_empty() && out.starts_with(&format!("{}/data/data.img: the entries of a text file", stage.display())) && out.contains("\ncrashes            3  chronod 2, Preferences 1\n"), "{status} {out} {err}");

    // what cannot be read ends with status 1 and no report; a wrong command line with status 2
    let other = work.join("other.img");
    std::fs::write(&other, vec![7u8; 8192]).unwrap();
    let gone = work.join("gone.txt");
    let (status, out, err) = health(&[other.to_str().unwrap(), "-o", gone.to_str().unwrap()]);
    assert!(status == 1 && out.is_empty() && err.contains("ioslog: ") && !gone.exists(), "{status} {out} {err}");
    assert_eq!(health(&[fixture, "--boot", "9A6A"]).0, 1);
    assert_eq!(health(&[fixture, "--top"]).0, 2);
    let _ = std::fs::remove_dir_all(&work);
}
