//! The lines of `oneios-tool serve` (README "serve"): an event as one JSON object whose first key is `ev`, the
//! writer every thread shares, and a request with its keys checked.

use serde_json::{Map, Value};

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The protocol's version (`hello`'s `proto`).
pub const PROTO: u32 = 1;
/// A `line` event's text is cut here.
pub const LINE_MAX: usize = 4000;

/// One line for the app: its keys in the order they were added (`ev` first: the app skips any line of the tool's
/// standard output that does not begin with `{"ev":`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ev(Vec<(String, Value)>);

impl Ev {
    pub fn new(ev: &str) -> Ev {
        Ev(vec![("ev".to_string(), Value::from(ev))])
    }

    /// An event of a request.
    pub fn of(ev: &str, id: &str) -> Ev {
        Ev::new(ev).f("id", id)
    }

    pub fn f(mut self, key: &str, value: impl Into<Value>) -> Ev {
        self.0.push((key.to_string(), value.into()));
        self
    }

    /// The key only when there is a value.
    pub fn opt(self, key: &str, value: Option<impl Into<Value>>) -> Ev {
        match value {
            Some(value) => self.f(key, value),
            None => self,
        }
    }

    pub fn fields(mut self, fields: &[(&str, Value)]) -> Ev {
        self.0.extend(fields.iter().map(|(k, v)| (k.to_string(), v.clone())));
        self
    }

    pub fn name(&self) -> &str {
        self.0[0].1.as_str().unwrap_or("")
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The line, without its end. serde_json writes no line break inside a value (a `\n` of a text is `\\n`).
    pub fn line(&self) -> String {
        let mut out = String::from("{");
        for (i, (key, value)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&Value::from(key.as_str()).to_string());
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push('}');
        out
    }
}

/// A float with one decimal, as a JSON number (`percent`, the temperatures).
pub fn one_decimal(value: f64) -> Value {
    Value::from((value * 10.0).round() / 10.0)
}

/// Where the lines go: the app's pipe. Every thread writes through it, one whole line at a time.
pub struct Out {
    to: Mutex<Box<dyn Write + Send>>,
    /// A write failed: the reader is gone.
    broken: AtomicBool,
    /// Called once when a write fails (wakes the main loop).
    on_broken: Box<dyn Fn() + Send + Sync>,
}

impl Out {
    pub fn new(to: Box<dyn Write + Send>, on_broken: Box<dyn Fn() + Send + Sync>) -> Out {
        Out { to: Mutex::new(to), broken: AtomicBool::new(false), on_broken }
    }

    pub fn send(&self, ev: &Ev) {
        if self.broken() {
            return;
        }
        let mut line = ev.line();
        line.push('\n');
        let mut to = self.to.lock().unwrap();
        if to.write_all(line.as_bytes()).and_then(|()| to.flush()).is_err() && !self.broken.swap(true, Ordering::SeqCst) {
            (self.on_broken)();
        }
    }

    pub fn broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }
}

/// How a request ends when it does not end well. `job`'s stop (a cancel, the heat limit) is found by the runner
/// itself: a handler just returns the error it got.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// The tool decided not to do it: a fixed code and the reason in plain words.
    Refused(&'static str, String),
    /// Something broke.
    Failed(&'static str, String),
}

impl Fail {
    pub fn usage(text: impl Into<String>) -> Fail {
        Fail::Refused("usage", text.into())
    }
    pub fn refused(code: &'static str, text: impl Into<String>) -> Fail {
        Fail::Refused(code, text.into())
    }
    pub fn failed(text: impl Into<String>) -> Fail {
        Fail::Failed("failed", text.into())
    }
    pub fn io(text: impl Into<String>) -> Fail {
        Fail::Failed("io", text.into())
    }
}

/// What a request that ended well says: one line for the screen and its own result fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    pub text: String,
    pub fields: Vec<(&'static str, Value)>,
}

impl Done {
    pub fn new(text: impl Into<String>) -> Done {
        Done { text: text.into(), fields: Vec::new() }
    }
    pub fn f(mut self, key: &'static str, value: impl Into<Value>) -> Done {
        self.fields.push((key, value.into()));
        self
    }
}

/// An id: 1 to 64 characters of `A-Z a-z 0-9 . _ -`.
pub fn id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A request: its id, its command, and its other keys, each of which the command must read (a key it does not
/// know is refused, so a typing mistake is not silently ignored).
#[derive(Debug)]
pub struct Req {
    pub id: String,
    pub cmd: String,
    keys: Map<String, Value>,
    read: Mutex<BTreeSet<String>>,
}

impl Req {
    /// A line as a request; `Err`: the text of an `error` line.
    pub fn parse(line: &str) -> Result<Req, String> {
        let value: Value = serde_json::from_str(line).map_err(|e| format!("not a JSON line: {e}"))?;
        let Value::Object(mut keys) = value else { return Err("not a JSON object".to_string()) };
        let id = match keys.remove("id") {
            Some(Value::String(id)) => id,
            Some(Value::Number(n)) if n.is_u64() => n.to_string(),
            Some(_) => return Err("\"id\" is not a text or a whole number".to_string()),
            None => return Err("no \"id\"".to_string()),
        };
        if !id_ok(&id) {
            return Err("\"id\" must be 1 to 64 characters of A-Z a-z 0-9 . _ -".to_string());
        }
        let cmd = match keys.remove("cmd") {
            Some(Value::String(cmd)) => cmd,
            _ => return Err(format!("request {id} has no \"cmd\"")),
        };
        Ok(Req { id, cmd, keys, read: Mutex::new(BTreeSet::new()) })
    }

    fn take(&self, key: &str) -> Option<&Value> {
        self.read.lock().unwrap().insert(key.to_string());
        self.keys.get(key).filter(|v| !v.is_null())
    }

    fn wrong(&self, key: &str, what: &str) -> Fail {
        Fail::usage(format!("{}: \"{key}\" must be {what}", self.cmd))
    }

    fn missing(&self, key: &str) -> Fail {
        Fail::usage(format!("{}: \"{key}\" is missing", self.cmd))
    }

    pub fn opt_str(&self, key: &str) -> Result<Option<String>, Fail> {
        match self.take(key) {
            None => Ok(None),
            Some(Value::String(text)) if !text.contains('\0') => Ok(Some(text.clone())),
            Some(_) => Err(self.wrong(key, "a text")),
        }
    }

    pub fn str(&self, key: &str) -> Result<String, Fail> {
        self.opt_str(key)?.ok_or_else(|| self.missing(key))
    }

    pub fn opt_u64(&self, key: &str) -> Result<Option<u64>, Fail> {
        match self.take(key) {
            None => Ok(None),
            Some(Value::Number(n)) if n.is_u64() => Ok(n.as_u64()),
            Some(_) => Err(self.wrong(key, "a whole number, 0 or more")),
        }
    }

    pub fn u64(&self, key: &str) -> Result<u64, Fail> {
        self.opt_u64(key)?.ok_or_else(|| self.missing(key))
    }

    pub fn opt_f64(&self, key: &str) -> Result<Option<f64>, Fail> {
        match self.take(key) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(_) => Err(self.wrong(key, "a number")),
        }
    }

    pub fn flag(&self, key: &str) -> Result<bool, Fail> {
        match self.take(key) {
            None => Ok(false),
            Some(Value::Bool(on)) => Ok(*on),
            Some(_) => Err(self.wrong(key, "true or false")),
        }
    }

    /// A list of texts (missing: empty).
    pub fn list(&self, key: &str) -> Result<Vec<String>, Fail> {
        match self.take(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items.iter().map(|v| v.as_str().filter(|t| !t.contains('\0')).map(str::to_string).ok_or_else(|| self.wrong(key, "a list of texts"))).collect(),
            Some(_) => Err(self.wrong(key, "a list of texts")),
        }
    }

    /// A list of objects, each with the text keys `names` (the first must be there; the others may be missing).
    pub fn objects(&self, key: &str, names: &[&str]) -> Result<Vec<Vec<Option<String>>>, Fail> {
        let what = format!("a list of objects with \"{}\"", names.join("\", \""));
        let Some(Value::Array(items)) = self.take(key) else { return Err(self.missing(key)) };
        items
            .iter()
            .map(|item| {
                let object = item.as_object().ok_or_else(|| self.wrong(key, &what))?;
                if object.keys().any(|k| !names.contains(&k.as_str())) || !object.get(names[0]).is_some_and(Value::is_string) {
                    return Err(self.wrong(key, &what));
                }
                names.iter().map(|name| match object.get(*name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(text)) if !text.contains('\0') => Ok(Some(text.clone())),
                    Some(_) => Err(self.wrong(key, &what)),
                }).collect()
            })
            .collect()
    }

    /// After a command read its keys: any other key is a mistake.
    pub fn done(&self) -> Result<(), Fail> {
        let read = self.read.lock().unwrap();
        match self.keys.keys().find(|k| !read.contains(*k)) {
            Some(key) => Err(Fail::usage(format!("{} has no key \"{key}\"", self.cmd))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_one_line_that_begins_with_ev() {
        let ev = Ev::of("progress", "7").f("step", "copy").f("done", 41943040u64).f("percent", one_decimal(22.86)).opt("eta", Some(1u64)).opt("rate", None::<f64>);
        assert_eq!(ev.line(), r#"{"ev":"progress","id":"7","step":"copy","done":41943040,"percent":22.9,"eta":1}"#);
        // a text with a line break, a quote and a path with a space stays one line
        let ev = Ev::of("line", "a.b-c_1").f("text", "two\nlines \"quoted\" /a b/c\t\u{1}");
        let line = ev.line();
        assert!(line.starts_with(r#"{"ev":"line","id":"a.b-c_1","text":"#) && !line.contains('\n'), "{line}");
        let back: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(back["text"], "two\nlines \"quoted\" /a b/c\t\u{1}");
        assert_eq!((ev.name(), ev.get("id").and_then(Value::as_str)), ("line", Some("a.b-c_1")));
    }

    #[test]
    fn a_request_names_its_keys() {
        let r = Req::parse(r#"{"id":"7","cmd":"sums","dir":"/a b","strict":true,"jobs":2,"list":null,"members":[{"member":"x","name":"y"},{"member":"z"}],"only":["a","b"]}"#).unwrap();
        assert_eq!((r.id.as_str(), r.cmd.as_str()), ("7", "sums"));
        assert_eq!(r.str("dir").unwrap(), "/a b");
        assert_eq!((r.flag("strict").unwrap(), r.flag("quick").unwrap()), (true, false));
        assert_eq!((r.opt_u64("jobs").unwrap(), r.opt_str("list").unwrap()), (Some(2), None));
        assert_eq!(r.objects("members", &["member", "name"]).unwrap(), vec![vec![Some("x".to_string()), Some("y".to_string())], vec![Some("z".to_string()), None]]);
        assert_eq!(r.list("only").unwrap(), ["a", "b"]);
        assert_eq!(r.done(), Ok(()));
        // a key the command does not read, a missing one, a wrong kind
        let r = Req::parse(r#"{"id":7,"cmd":"sums","dri":"/a","dir":5}"#).unwrap();
        assert_eq!(r.id, "7");
        assert_eq!(r.str("dir"), Err(Fail::usage("sums: \"dir\" must be a text")));
        assert_eq!(r.str("list"), Err(Fail::usage("sums: \"list\" is missing")));
        assert_eq!(r.done(), Err(Fail::usage("sums has no key \"dri\"")));
        assert_eq!(Req::parse(r#"{"id":"1","cmd":"x","n":-1}"#).unwrap().opt_u64("n"), Err(Fail::usage("x: \"n\" must be a whole number, 0 or more")));
        assert!(Req::parse(r#"{"id":"1","cmd":"x","m":[{"name":"y"}]}"#).unwrap().objects("m", &["member", "name"]).is_err());
        assert!(Req::parse(r#"{"id":"1","cmd":"x","m":[{"member":"y","other":"z"}]}"#).unwrap().objects("m", &["member", "name"]).is_err());
    }

    #[test]
    fn a_line_that_is_not_a_request() {
        for (line, why) in [
            ("hello", "not a JSON line"),
            ("[1]", "not a JSON object"),
            (r#"{"cmd":"ping"}"#, "no \"id\""),
            (r#"{"id":"a b","cmd":"ping"}"#, "1 to 64 characters"),
            (r#"{"id":"","cmd":"ping"}"#, "1 to 64 characters"),
            (r#"{"id":true,"cmd":"ping"}"#, "not a text or a whole number"),
            (r#"{"id":"1"}"#, "has no \"cmd\""),
        ] {
            let e = Req::parse(line).unwrap_err();
            assert!(e.contains(why), "{line}: {e}");
        }
        assert!(id_ok(&"a".repeat(64)) && !id_ok(&"a".repeat(65)) && !id_ok("a/b") && id_ok("A.z_0-9"));
    }

    #[test]
    fn a_broken_pipe_is_said_once() {
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let said = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = said.clone();
        let out = Out::new(Box::new(Gone), Box::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        out.send(&Ev::new("hello"));
        out.send(&Ev::new("bye"));
        assert!(out.broken());
        assert_eq!(said.load(Ordering::SeqCst), 1);
    }
}
