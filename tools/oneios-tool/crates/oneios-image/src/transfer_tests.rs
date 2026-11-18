//! transfer.rs's tests: the byte ranges, the waits, the state file and when it is gone on from, and downloads from
//! `rangeserve` in this process as a bad network (its options: drops in the middle of a body, 503s, no range
//! requests, another ETag). The pause, the cancel and the events are tested through the process
//! (crates/oneios-tool/tests/serve.rs): they are `job`'s state, which is the process's.

use super::*;
use crate::fixtures::{noise, Tmp};
use crate::rangeserve::{serve_on, Faults};

/// The tests that use the network share the process's give-up time and its failure word: one at a time.
static NET: Mutex<()> = Mutex::new(());

/// `rangeserve` on a free port of 127.0.0.1, in a thread that lasts as long as the test process.
fn web(root: &str, faults: Faults) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let root = std::path::PathBuf::from(root);
    std::thread::spawn(move || serve_on(listener, root, faults));
    format!("http://127.0.0.1:{port}")
}

fn get(url: &str, out: &str, connections: usize, piece: u64) -> (Result<(u64, Fetched)>, Vec<String>) {
    let log = Mutex::new(Vec::new());
    let done = download(url, out, connections, piece, None, &|_| Ok(()), &|line| log.lock().unwrap().push(line));
    (done, log.into_inner().unwrap())
}

#[test]
fn ranges_merge_and_say_what_is_missing() {
    let mut r = Ranges::default();
    assert_eq!((r.bytes(), r.prefix(), r.gaps(100)), (0, 0, vec![(0, 100)]));
    r.add(10, 20);
    r.add(40, 50);
    r.add(20, 30); // touches the first
    r.add(5, 5); // nothing
    assert_eq!(r.0, [(10, 30), (40, 50)]);
    assert_eq!((r.bytes(), r.prefix(), r.gaps(100)), (30, 0, vec![(0, 10), (30, 40), (50, 100)]));
    r.add(0, 12);
    assert_eq!((r.0.clone(), r.prefix()), (vec![(0, 30), (40, 50)], 30));
    r.add(25, 60); // over a gap and two ranges
    assert_eq!((r.0.clone(), r.gaps(60)), (vec![(0, 60)], vec![]));
    r.add(70, 80);
    r.add(60, 70);
    assert_eq!(r.0, [(0, 80)]);
    // a state file's list: pairs in order and apart, inside the length
    let list = |text: &str| Ranges::of_list(&serde_json::from_str(text).unwrap(), 100);
    assert_eq!(list("[[0,10],[20,100]]"), Some(Ranges(vec![(0, 10), (20, 100)])));
    assert_eq!(list("[]"), Some(Ranges::default()));
    for bad in ["[[0,10],[10,20]]", "[[20,30],[0,10]]", "[[0,101]]", "[[5,5]]", "[[0]]", "[[0,1,2]]", "[0,10]", "{}", "[[\"0\",10]]"] {
        assert_eq!(list(bad), None, "{bad}");
    }
}

#[test]
fn the_wait_doubles_from_a_second_to_a_minute_less_a_random_part() {
    for (tries, whole) in [(1, 1.0), (2, 2.0), (3, 4.0), (6, 32.0), (7, 60.0), (8, 60.0), (40, 60.0)] {
        let waits: Vec<f64> = (0..200).map(|_| backoff(tries).as_secs_f64()).collect();
        assert!(waits.iter().all(|w| (whole / 2.0..=whole).contains(w)), "try {tries}: {waits:?}");
        // (a random part: 200 waits are not one wait)
        assert!(waits.iter().cloned().fold(0.0, f64::max) - waits.iter().cloned().fold(f64::MAX, f64::min) > whole / 10.0, "try {tries}");
    }
}

#[test]
fn the_pieces_are_few_and_large() {
    let mib = 1u64 << 20;
    assert_eq!(piece_size(5 * mib, 8), 8 * mib, "never under 8 MiB");
    assert_eq!(piece_size(1024 * mib, 8), 32 * mib, "1 GiB by 8 connections: 32 pieces");
    assert_eq!(piece_size(1024 * mib, 1), 256 * mib);
    assert_eq!(piece_size(100_000 * mib, 8), 256 * mib, "never over 256 MiB");
    assert_eq!(piece_size(1000 * mib + 1, 4) % mib, 0, "whole MiB");
}

#[test]
fn the_state_file_and_when_it_is_gone_on_from() {
    let tmp = Tmp::new("transfer-state");
    let (part, state) = (tmp.write("x.part", &vec![7u8; 1000]), tmp.path("x.part.state"));
    let ident = Ident { source: "https://host/xnu.macho", what: "file", size: 1000, etag: Some("\"6917a5c1-3e8\""), last_modified: Some("Sat, 14 Nov 2026 21:41:21 GMT"), offset: 0, length: 1000, ranges: true };
    let mut done = Ranges::default();
    done.add(0, 600);
    done.add(700, 800);
    // the line, its keys in the README's order
    let line = ident.line(&done);
    assert_eq!(
        line,
        "{\"v\":1,\"source\":\"https://host/xnu.macho\",\"what\":\"file\",\"size\":1000,\"etag\":\"\\\"6917a5c1-3e8\\\"\",\"last_modified\":\"Sat, 14 Nov 2026 21:41:21 GMT\",\"offset\":0,\"length\":1000,\"ranges\":true,\"done\":[[0,600],[700,800]]}\n"
    );
    // nothing there: nothing to go on from, and nothing to say
    assert_eq!(ident.earlier(&tmp.path("none.part.state"), &tmp.path("none.part")), Ok(Ranges::default()));
    assert_eq!(ident.earlier(&state, &part), Err("its part has no state file".to_string()));
    ident.save(&File::open(&part).unwrap(), &state, &done).unwrap();
    assert_eq!(std::fs::read_to_string(&state).unwrap(), line);
    assert!(!std::path::Path::new(&format!("{state}.tmp")).exists(), "written under another name, then renamed");
    assert_eq!(ident.earlier(&state, &part), Ok(done.clone()));
    // another file on the server, another download, another part: each starts again and says why
    let other = |change: &dyn Fn(&mut Ident)| {
        let mut other = Ident { ..ident };
        change(&mut other);
        other.earlier(&state, &part).unwrap_err()
    };
    assert_eq!(other(&|i| i.etag = Some("\"b\"")), "the server's file changed (ETag \"6917a5c1-3e8\" then, \"b\" now)");
    assert_eq!(other(&|i| i.etag = None), "the server's file changed (ETag \"6917a5c1-3e8\" then, none now)");
    assert_eq!(other(&|i| i.last_modified = Some("Sun, 15 Nov 2026 08:00:00 GMT")), "the server's file changed (Last-Modified Sat, 14 Nov 2026 21:41:21 GMT then, Sun, 15 Nov 2026 08:00:00 GMT now)");
    assert_eq!(other(&|i| i.size = 1001), "the server's file changed (1000 bytes then, 1001 now)");
    assert_eq!(other(&|i| i.source = "https://host/other"), "its part is of another download (file of https://host/xnu.macho)");
    assert_eq!(other(&|i| i.what = "member 00000000 a"), "its part is of another download (file of https://host/xnu.macho)");
    assert_eq!(other(&|i| i.offset = 4), "its part is of another download (file of https://host/xnu.macho)");
    // (whether the server has ranges is not what the part is of: a part of ranges is gone on from either way)
    assert_eq!(Ident { ranges: false, ..ident }.earlier(&state, &part), Ok(done.clone()));
    std::fs::write(&part, vec![7u8; 999]).unwrap();
    assert_eq!(ident.earlier(&state, &part), Err("its part has 999 bytes, not 1000".to_string()));
    std::fs::write(&part, vec![7u8; 1000]).unwrap();
    for (text, why) in [("not json", "the state file cannot be read"), ("{\"v\":2}", "another version"), (&line.replace("[700,800]", "[700,2000]"), "list of ranges")] {
        std::fs::write(&state, text).unwrap();
        let said = ident.earlier(&state, &part).unwrap_err();
        assert!(said.contains(why), "{text}: {said}");
    }
}

#[test]
fn drops_and_503s_are_tried_again_from_the_byte_they_reached() {
    let _net = NET.lock().unwrap();
    begin_request(GIVE_UP_DEFAULT);
    let tmp = Tmp::new("transfer-drops");
    let data = noise(3 << 20, 5);
    tmp.write("web/a.bin", &data);
    // every body breaks off after 300 to 900 KB, and every 6th request is answered 503
    let url = format!("{}/a.bin", web(&tmp.path("web"), Faults { drop_every: 600_000, fail_every: 6, ..Faults::default() }));
    let (done, log) = get(&url, &tmp.path("a.bin"), 3, 1 << 20);
    let (size, fetched) = done.unwrap_or_else(|e| panic!("{e}: {log:?}"));
    assert_eq!((size, fetched.resumed_from, fetched.ranges, fetched.connections), (3 << 20, 0, true, 3));
    assert!(fetched.retries >= 3, "three pieces of 1 MiB, each longer than a body lasts: {fetched:?}");
    assert_eq!(std::fs::read(tmp.path("a.bin.part")).unwrap(), data);
    // the state lists the part whole; a second call has nothing to fetch and says so
    let state: Value = serde_json::from_str(&std::fs::read_to_string(tmp.path("a.bin.part.state")).unwrap()).unwrap();
    assert_eq!((state["done"].to_string(), state["what"].as_str(), state["ranges"].as_bool()), ("[[0,3145728]]".to_string(), Some("file"), Some(true)));
    assert!(state["etag"].is_string() && state["last_modified"].as_str().is_some_and(|t| t.ends_with(" GMT")), "{state}");
    let (done, log) = get(&url, &tmp.path("a.bin"), 3, 1 << 20);
    assert_eq!(done.unwrap().1.resumed_from, 3 << 20);
    assert!(log.contains(&"a.bin: going on from 3145728 of 3145728 bytes".to_string()), "{log:?}");
    assert_eq!(take_failure(), None);
}

#[test]
fn a_part_is_gone_on_from_only_when_the_servers_file_is_the_same() {
    let _net = NET.lock().unwrap();
    begin_request(GIVE_UP_DEFAULT);
    let tmp = Tmp::new("transfer-etag");
    let data = noise(600_000, 6);
    tmp.write("web/a.bin", &data);
    let half = 300_000u64;
    // what an interrupted run against a server with the ETag "one" left: the first half, and its state
    let before = |out: &str, source: &Source| {
        let mut part = data.clone();
        part[half as usize..].fill(0);
        tmp.write(&format!("{out}.part"), &part);
        tmp.write(&format!("{out}.part.state"), source.state_line("file", 0, data.len() as u64, &[(0, half)]).as_bytes());
    };
    let one = format!("{}/a.bin", web(&tmp.path("web"), Faults { etag: Some("one".into()), ..Faults::default() }));
    let source = Source::open(&one).unwrap();
    assert_eq!((source.size, source.etag.as_deref()), (600_000, Some("\"one\"")));
    // the same server: the second half only
    before("same", &source);
    let (done, log) = get(&one, &tmp.path("same"), 2, 100_000);
    assert_eq!(done.unwrap().1.resumed_from, half, "{log:?}");
    assert_eq!(log.iter().filter(|line| line.contains("% (")).count(), 3, "three pieces of 100,000 bytes were fetched: {log:?}");
    assert_eq!(std::fs::read(tmp.path("same.part")).unwrap(), data);
    // the file changed on the server (its ETag is another): from the first byte, and a line says why
    let two = format!("{}/a.bin", web(&tmp.path("web"), Faults { etag: Some("two".into()), ..Faults::default() }));
    before("other", &source);
    std::fs::write(tmp.path("other.part.state"), std::fs::read_to_string(tmp.path("other.part.state")).unwrap().replace(&one, &two)).unwrap();
    let (done, log) = get(&two, &tmp.path("other"), 2, 100_000);
    assert_eq!(done.unwrap().1.resumed_from, 0);
    assert!(log.contains(&"a.bin: the server's file changed (ETag \"one\" then, \"two\" now): starting again from the first byte".to_string()), "{log:?}");
    assert_eq!(std::fs::read(tmp.path("other.part")).unwrap(), data);
    // a server that names neither an ETag nor a Last-Modified: the size alone, and a line says so
    let bare = format!("{}/a.bin", web(&tmp.path("web"), Faults { etag: Some("none".into()), ..Faults::default() }));
    let source = Source::open(&bare).unwrap();
    assert_eq!((source.etag.as_deref(), source.last_modified.as_deref()), (None, None));
    before("bare", &source);
    let (done, log) = get(&bare, &tmp.path("bare"), 2, 100_000);
    assert_eq!(done.unwrap().1.resumed_from, half);
    assert!(log.iter().any(|line| line.contains("only the size says that the file is the same")), "{log:?}");
}

#[test]
fn a_server_without_ranges_is_one_stream_from_its_first_byte() {
    let _net = NET.lock().unwrap();
    begin_request(GIVE_UP_DEFAULT);
    let tmp = Tmp::new("transfer-norange");
    let data = noise(2 << 20, 7);
    tmp.write("web/a.bin", &data);
    // (its bodies break off too: each time the stream begins again, and gets further than half only sometimes)
    let url = format!("{}/a.bin", web(&tmp.path("web"), Faults { no_range: true, drop_every: 2_500_000, ..Faults::default() }));
    // a part of an earlier run cannot be gone on from
    let source = Source::open(&url).unwrap();
    tmp.write("a.bin.part", &vec![0u8; data.len()]);
    tmp.write("a.bin.part.state", source.state_line("file", 0, data.len() as u64, &[(0, 1000)]).as_bytes());
    let (done, log) = get(&url, &tmp.path("a.bin"), 4, 1 << 20);
    let (_, fetched) = done.unwrap_or_else(|e| panic!("{e}: {log:?}"));
    assert_eq!((fetched.ranges, fetched.connections, fetched.resumed_from), (false, 1, 0), "{log:?}");
    assert!(log.iter().any(|line| line.contains("the server has no range requests: one stream")), "{log:?}");
    assert!(log.contains(&"a.bin: the server has no range requests: starting again from the first byte".to_string()), "{log:?}");
    assert_eq!(std::fs::read(tmp.path("a.bin.part")).unwrap(), data);
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(tmp.path("a.bin.part.state")).unwrap()).unwrap()["ranges"], false);
    // members of an archive need ranges
    let error = source.read(10, 20).unwrap_err();
    assert!(error.contains("the server ignores range requests (status 200)"), "{error}");
}

#[test]
fn it_gives_up_when_no_byte_arrives_for_the_time_it_was_given() {
    let _net = NET.lock().unwrap();
    let tmp = Tmp::new("transfer-giveup");
    tmp.write("web/a.bin", b"x");
    let url = format!("{}/a.bin", web(&tmp.path("web"), Faults { fail_first: u64::MAX, ..Faults::default() }));
    begin_request(2);
    let began = Instant::now();
    let error = Source::open(&url).err().expect("a server that answers 503 every time");
    assert!(error.contains("no byte arrived for 2 s: gave up (the server answered 503"), "{error}");
    assert!((2..6).contains(&began.elapsed().as_secs()), "{:?}", began.elapsed());
    assert_eq!(take_failure(), Some("network"));
    // a status that another try would not change ends at once
    begin_request(GIVE_UP_DEFAULT);
    let url = format!("{}/missing.bin", web(&tmp.path("web"), Faults::default()));
    let began = Instant::now();
    let error = Source::open(&url).err().expect("404");
    assert!(error.contains("status 404") && began.elapsed() < Duration::from_secs(1), "{error}");
    assert_eq!(take_failure(), None);
}
