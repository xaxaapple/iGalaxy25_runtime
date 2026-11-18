//! `oneios-tool app`: the OneOS fork's app (docs/building.md section 7): where its source is, its version, the
//! build, the prerequisites, the install, and the fork's three scripts into the stage.
//!
//! **Not built and not installed for real yet.** The owner's decision for the box this was written on: no app
//! build there, and no JDK 21 or Android SDK for it. It is tested with a stand-in gradlew, gh and adb
//! (`tests/app/run.sh`); `app version` was run on the real clone.

use crate::adb::{last_lines, plain, Adb, STAGE};
use crate::phone::{installed_app, APP_ID};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a folder must hold to be the fork's checkout, whatever its name: the app's build file (the version is
/// read from it) and the fork's own switch script (upstream OneOS has no `oneios_switch.sh`).
const MARKS: [&str; 2] = ["app-android/app/build.gradle.kts", "switcher/assets/oneios_switch.sh"];
/// What fills an `app/` that a clone without `--recurse-submodules`, or a new worktree, left empty.
pub const SUBMODULE: &str = "git submodule update --init --remote app";
const APK: &str = "app-android/app/build/outputs/apk/full/debug/app-full-debug.apk";
/// The fork's three scripts and where each goes in the stage.
const SCRIPTS: [(&str, &str); 3] = [
    ("switcher/assets/oneios_switch.sh", "oneios_switch.sh"),
    ("switcher/assets/oneios_return.sh", "oneios_return.sh"),
    ("app-android/app/src/full/assets/root/oneios_root.sh", "bin/oneios_root.sh"),
];

pub struct Fork {
    pub dir: PathBuf,
    pub how: &'static str,
}

/// What a folder is to the lookup.
#[derive(Debug, PartialEq)]
enum Folder {
    Checkout,
    NotThere,
    /// a folder with nothing in it: what a submodule that is not initialised looks like
    Empty,
    /// the first of MARKS it does not have
    Lacks(&'static str),
}

fn folder(dir: &Path) -> Folder {
    if !dir.is_dir() {
        return Folder::NotThere;
    }
    match MARKS.iter().find(|m| !dir.join(m).is_file()) {
        None => Folder::Checkout,
        Some(_) if fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false) => Folder::Empty,
        Some(m) => Folder::Lacks(m),
    }
}

/// Where the lookup looks: everything it reads from the command line, the environment and git, so that the
/// order itself (`look`) can be tested on made-up folders.
pub struct Places {
    /// `--oneos DIR`
    pub option: Option<String>,
    /// `ONEOS_DIR`
    pub env: Option<String>,
    /// the current folder
    pub here: PathBuf,
    /// the OneiOS checkout the command runs in (`cycle::repo_root`), if any
    pub root: Option<PathBuf>,
    /// the main checkout of the git checkout at `root` (else at `here`): `root` itself unless it is a worktree
    pub main: Option<PathBuf>,
}

impl Places {
    pub fn now(option: Option<&str>) -> Result<Places> {
        let here = std::env::current_dir().context("the current folder")?;
        let root = crate::cycle::repo_root().ok();
        // <main checkout>/.git -> <main checkout>, found through git's common folder, so it works from a worktree
        let main = Command::new("git")
            .arg("-C")
            .arg(root.as_deref().unwrap_or(&here))
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| Path::new(String::from_utf8_lossy(&o.stdout).trim()).parent().map(Path::to_path_buf))
            .filter(|p| p.is_dir());
        Ok(Places { option: option.map(str::to_string), env: std::env::var("ONEOS_DIR").ok().filter(|d| !d.is_empty()), here, root, main })
    }
}

/// The OneOS checkout and what to tell the user about the places passed over. The order:
///
/// 1. `--oneos DIR`, 2. `ONEOS_DIR`: a checkout named by hand. If it is not one, that is an error: the lookup
///    never goes on to another place behind the user's back.
/// 3. the current folder, when it is a checkout (the user stands in it);
/// 4. `app/` of the OneiOS checkout: the submodule, the default;
/// 5. `app/` of the OneiOS main checkout, when the command runs in a worktree (a new worktree's `app/` is empty);
/// 6. `OneOS` beside the OneiOS main checkout: the clone from before the submodule.
///
/// An `app/` that is there but is no checkout (empty: the submodule is not initialised) is passed over with a
/// note that names `SUBMODULE`; when nothing is found the error lists every place and why it was not taken.
pub fn look(p: &Places) -> Result<(Fork, Vec<String>)> {
    let same = |a: &Path, b: &Path| matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b);
    let app = p.root.as_ref().map(|r| r.join("app"));
    let named = |dir: &str, how: &'static str| -> Result<Fork> {
        let dir = fs::canonicalize(dir).with_context(|| format!("{how}: {dir}"))?;
        match folder(&dir) {
            Folder::Checkout => Ok(Fork { dir, how }),
            Folder::Empty => bail!("{how}: {} is an empty folder, not a OneOS checkout (if it is OneiOS's app/, the submodule is not initialised: {SUBMODULE})", dir.display()),
            Folder::Lacks(mark) => bail!("{how}: {} is not a OneOS checkout (no {mark})", dir.display()),
            Folder::NotThere => bail!("{how}: {} is not a folder", dir.display()),
        }
    };
    if let Some(dir) = &p.option {
        return Ok((named(dir, "--oneos")?, Vec::new()));
    }
    if let Some(dir) = &p.env {
        let fork = named(dir, "ONEOS_DIR")?;
        // the environment is easy to forget: say so when it takes the place of the submodule
        let notes = app.iter().filter(|a| folder(a) == Folder::Checkout && !same(a, &fork.dir)).map(|a| format!("ONEOS_DIR is set, so {} is not used", a.display())).collect();
        return Ok((fork, notes));
    }
    if folder(&p.here) == Folder::Checkout {
        return Ok((Fork { dir: p.here.clone(), how: "the current folder" }, Vec::new()));
    }
    let mut places: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(app) = app {
        places.push((app, "app/ of the OneiOS checkout"));
    }
    if let Some(main) = &p.main {
        if !p.root.as_ref().is_some_and(|r| same(r, main)) {
            places.push((main.join("app"), "app/ of the OneiOS main checkout"));
        }
        if let Some(beside) = main.parent() {
            places.push((beside.join("OneOS"), "beside the OneiOS checkout"));
        }
    }
    // (why a place was not taken, whether that is worth a note when a later place is taken)
    let mut passed: Vec<(String, bool)> = Vec::new();
    for (dir, how) in places {
        let submodule = dir.file_name().is_some_and(|n| n == "app");
        let at = dir.parent().unwrap_or(Path::new(".")).display().to_string();
        match folder(&dir) {
            Folder::Checkout => return Ok((Fork { dir, how }, passed.into_iter().filter(|p| p.1).map(|p| p.0).collect())),
            Folder::NotThere => passed.push((format!("{} is not there", dir.display()), false)),
            Folder::Empty if submodule => passed.push((format!("{} is empty: the app submodule is not initialised (in {at}: {SUBMODULE})", dir.display()), true)),
            Folder::Empty => passed.push((format!("{} is empty", dir.display()), true)),
            Folder::Lacks(mark) if submodule => passed.push((format!("{} is not the fork's checkout (no {mark}; in {at}: {SUBMODULE})", dir.display()), true)),
            Folder::Lacks(mark) => passed.push((format!("{} is not the fork's checkout (no {mark})", dir.display()), true)),
        }
    }
    if passed.is_empty() {
        passed.push(("the current folder is not inside a OneiOS checkout (or ONEIOS_ROOT=), so there is no app/ to look in".to_string(), false));
    }
    bail!(
        "no OneOS checkout (a folder with {} and {}): {}. Name one with --oneos DIR or ONEOS_DIR, or get the submodule (in the OneiOS checkout: {SUBMODULE})",
        MARKS[0],
        MARKS[1],
        passed.into_iter().map(|p| p.0).collect::<Vec<_>>().join("; ")
    )
}

/// The OneOS checkout (`look` has the order); the notes about the places passed over go to stderr.
pub fn find_fork(option: Option<&str>) -> Result<Fork> {
    let (fork, notes) = look(&Places::now(option)?)?;
    for note in notes {
        eprintln!("note: {note}");
    }
    Ok(fork)
}

fn say_fork(fork: &Fork) {
    println!("OneOS checkout: {} ({})", fork.dir.display(), fork.how);
}

pub struct Version {
    pub id: String,
    pub name: String,
    pub code: u64,
    /// `oneos.series` of gradle.properties: the full flavour's versionNameSuffix
    pub series: Option<String>,
}

/// versionName, versionCode and applicationId of app-android/app/build.gradle.kts.
pub fn source_version(fork: &Fork) -> Result<Version> {
    let path = fork.dir.join("app-android/app/build.gradle.kts");
    let text = fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let quoted = |key: &str| -> Option<String> {
        text.lines().find_map(|l| l.trim().strip_prefix(key)?.trim().strip_prefix("= \"")?.strip_suffix('"').map(str::to_string))
    };
    let code = text.lines().find_map(|l| l.trim().strip_prefix("versionCode")?.trim().strip_prefix('=')?.trim().parse().ok());
    let series = fs::read_to_string(fork.dir.join("app-android/gradle.properties"))
        .ok()
        .and_then(|p| p.lines().find_map(|l| l.trim().strip_prefix("oneos.series=").map(|v| v.trim().to_string())))
        .filter(|v| !v.is_empty());
    Ok(Version {
        id: quoted("applicationId").ok_or_else(|| anyhow!("{}: no applicationId", path.display()))?,
        name: quoted("versionName").ok_or_else(|| anyhow!("{}: no versionName", path.display()))?,
        code: code.ok_or_else(|| anyhow!("{}: no versionCode", path.display()))?,
        series,
    })
}

fn git(fork: &Fork, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(&fork.dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The SHA-256 of each signing certificate of an APK, sorted: of the newest scheme it carries (v3.x, v3, v2,
/// else v1). Read with the `apk-info-zip` crate.
pub fn apk_certs(path: &Path) -> Result<Vec<String>> {
    use apk_info_zip::Signature as S;
    let bytes = fs::read(path).with_context(|| path.display().to_string())?;
    let zip = apk_info_zip::ZipEntry::new(bytes).map_err(|e| anyhow!("{}: not an APK: {e}", path.display()))?;
    let mut found: Vec<(u8, Vec<String>)> = Vec::new();
    let mut take = |rank: u8, certs: &[apk_info_zip::CertificateInfo]| found.push((rank, certs.iter().map(|c| c.sha256_fingerprint.to_lowercase().replace(':', "")).collect()));
    for s in zip.get_signatures_other().map_err(|e| anyhow!("{}: its signing block: {e}", path.display()))? {
        match &s {
            S::V32(c) => take(5, c),
            S::V31(c) => take(4, c),
            S::V3(c) => take(3, c),
            S::V2(c) => take(2, c),
            _ => {}
        }
    }
    if let Ok(Some(S::V1(c))) = zip.get_signature_v1() {
        take(1, &c);
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut certs = found.into_iter().map(|(_, certs)| certs).find(|c| !c.is_empty()).ok_or_else(|| anyhow!("{}: no signing certificate found", path.display()))?;
    certs.sort();
    certs.dedup();
    Ok(certs)
}

/// The installed app's APK, copied from the phone (`pm path`, then that one file), and its certificates.
pub fn installed_certs(adb: &Adb, id: &str) -> Result<Vec<String>> {
    plain(id, "the application id")?;
    let r = adb.shell(&format!("pm path {id}"))?;
    let path = r.out.lines().find_map(|l| l.trim().strip_prefix("package:")).ok_or_else(|| anyhow!("pm path {id} names no APK: {}", last_lines(&r.out, 1)))?;
    if !path.ends_with(".apk") || !path.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-/~=".contains(&b)) {
        bail!("pm path {id} gave a path this tool does not read: {path}");
    }
    let local = std::env::temp_dir().join(format!("oneios-tool-installed-{}.apk", std::process::id()));
    let mut child = adb.exec_out(&format!("cat '{path}'"))?;
    let mut file = fs::File::create(&local).with_context(|| local.display().to_string())?;
    std::io::copy(child.stdout.as_mut().expect("piped"), &mut file).context("copying the installed APK")?;
    let status = child.wait()?;
    let certs = if status.success() { apk_certs(&local) } else { Err(anyhow!("adb exec-out cat {path} ended with {status}")) };
    let _ = fs::remove_file(&local);
    certs
}

fn file_facts(path: &Path) -> Result<(u64, String)> {
    let bytes = fs::read(path).with_context(|| path.display().to_string())?;
    Ok((bytes.len() as u64, oneios_common::hex(&Sha256::digest(&bytes))))
}

// ---------------------------------------------------------------- version

pub fn version(fork: &Fork, serial: Option<&str>, no_phone: bool) -> Result<()> {
    say_fork(fork);
    let v = source_version(fork)?;
    println!("applicationId: {} (the full flavour; probe: {}.probe)", v.id, v.id);
    let full = v.series.as_ref().map(|s| format!("{}-{s}", v.name)).unwrap_or_else(|| v.name.clone());
    println!("versionName: {} (full: {full}, probe: {}-probe)", v.name, v.name);
    println!("versionCode: {}", v.code);
    let commit = git(fork, &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let branch = git(fork, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let changed = git(fork, &["status", "--porcelain"]).map(|s| s.lines().count());
    println!(
        "commit: {commit} (branch {branch}), {}",
        match changed {
            Some(0) => "clean".to_string(),
            Some(n) => format!("dirty ({n} changed files)"),
            None => "state unknown".to_string(),
        }
    );
    if no_phone {
        return Ok(());
    }
    match Adb::connect(serial) {
        Err(e) => println!("installed: not asked ({e:#})"),
        Ok(adb) => match installed_app(&adb, &v.id)? {
            None => println!("installed: {} is not installed on {}", v.id, adb.serial),
            Some((code, name)) => {
                println!("installed: {name} (versionCode {code}) on {}{}", adb.serial, if code == v.code { "" } else { ": not the source's versionCode" });
                match installed_certs(&adb, &v.id) {
                    Ok(certs) => println!("installed certificate sha256: {}", certs.join(", ")),
                    Err(e) => println!("installed certificate: not read ({e:#})"),
                }
            }
        },
    }
    Ok(())
}

// ---------------------------------------------------------------- build

fn apk_report(fork: &Fork, apk: &Path) -> Result<()> {
    let (size, sha) = file_facts(apk)?;
    let v = source_version(fork)?;
    println!("APK: {}", apk.display());
    println!("size: {size}");
    println!("sha256: {sha}");
    println!("version: {} (versionCode {})", v.name, v.code);
    match apk_certs(apk) {
        Ok(certs) => println!("certificate sha256: {}", certs.join(", ")),
        Err(e) => println!("certificate: not read ({e:#})"),
    }
    Ok(())
}

pub fn build(fork: &Fork, no_test: bool) -> Result<()> {
    say_fork(fork);
    let dir = fork.dir.join("app-android");
    let mut tasks = vec!["test", "assembleFullDebug"];
    if no_test {
        tasks.remove(0);
    }
    println!("cd {} && ./gradlew {}", dir.display(), tasks.join(" "));
    let status = Command::new("./gradlew").args(&tasks).current_dir(&dir).status().with_context(|| format!("cannot start {}/gradlew", dir.display()))?;
    if !status.success() {
        bail!("gradlew ended with {status} (prerequisites: oneios-tool app doctor)");
    }
    apk_report(fork, &fork.dir.join(APK))
}

// ---------------------------------------------------------------- doctor

struct Checks {
    failed: usize,
}

impl Checks {
    fn line(&mut self, ok: bool, needed: bool, what: &str, detail: &str) {
        let mark = if ok {
            "ok     "
        } else if needed {
            self.failed += 1;
            "MISSING"
        } else {
            "absent "
        };
        println!("{mark} {what}: {detail}");
    }
}

fn java_major(java: &Path) -> Option<(u32, String)> {
    let out = Command::new(java).arg("-version").output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    let version = text.split('"').nth(1)?.to_string();
    let mut parts = version.split(['.', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    let major = if first == 1 { parts.next()?.parse().ok()? } else { first };
    Some((major, version))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// A `key=value` of a Java properties file (no escapes: the two keys read here are a login and a token).
fn property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(['=', ':'])?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

pub fn doctor(fork: &Fork) -> Result<()> {
    say_fork(fork);
    let mut c = Checks { failed: 0 };
    let gradlew = fork.dir.join("app-android/gradlew");
    c.line(gradlew.is_file(), true, "gradlew", &gradlew.display().to_string());
    // JDK 21: JAVA_HOME's, else the one on PATH
    let java = std::env::var("JAVA_HOME").ok().filter(|h| !h.is_empty()).map(|h| Path::new(&h).join("bin/java")).unwrap_or_else(|| PathBuf::from("java"));
    match java_major(&java) {
        Some((21, v)) => c.line(true, true, "JDK 21", &format!("{} is {v}", java.display())),
        Some((_, v)) => c.line(false, true, "JDK 21", &format!("{} is {v}; the build's toolchain is 21 (set JAVA_HOME to a JDK 21)", java.display())),
        None => c.line(false, true, "JDK 21", &format!("{} does not run (install a JDK 21, set JAVA_HOME)", java.display())),
    }
    // the Android SDK with platform 37
    let local = fs::read_to_string(fork.dir.join("app-android/local.properties")).ok().and_then(|t| property(&t, "sdk.dir"));
    let sdks: Vec<(String, &str)> = [
        (std::env::var("ANDROID_HOME").ok(), "ANDROID_HOME"),
        (std::env::var("ANDROID_SDK_ROOT").ok(), "ANDROID_SDK_ROOT"),
        (local, "local.properties sdk.dir"),
        (Some(home().join("Library/Android/sdk").display().to_string()), "~/Library/Android/sdk"),
        (Some(home().join("Android/Sdk").display().to_string()), "~/Android/Sdk"),
    ]
    .into_iter()
    .filter_map(|(dir, how)| dir.filter(|d| !d.is_empty() && Path::new(d).join("platforms").is_dir()).map(|d| (d, how)))
    .collect();
    match sdks.first() {
        None => c.line(false, true, "Android SDK", "none found (ANDROID_HOME, ANDROID_SDK_ROOT, app-android/local.properties sdk.dir, ~/Library/Android/sdk, ~/Android/Sdk)"),
        Some((dir, how)) => {
            c.line(true, true, "Android SDK", &format!("{dir} ({how})"));
            let platform = fs::read_dir(Path::new(dir).join("platforms"))
                .ok()
                .and_then(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n == "android-37" || n.starts_with("android-37.")));
            match platform {
                Some(name) => c.line(true, true, "platform 37", &format!("platforms/{name}")),
                None => c.line(false, true, "platform 37", "not in the SDK (sdkmanager \"platforms;android-37.0\" \"build-tools;37.0.0\")"),
            }
        }
    }
    // the GitHub Packages login: the file first, as settings.gradle.kts reads it; a token is never printed
    let file = home().join(".config/twidget/github.properties");
    let text = fs::read_to_string(&file).unwrap_or_default();
    let (user, user_from) = match property(&text, "ghUsername") {
        Some(u) => (u, "github.properties"),
        None => (std::env::var("GH_PACKAGES_USER").unwrap_or_default(), "GH_PACKAGES_USER"),
    };
    let (token, token_from) = match property(&text, "ghAccessToken") {
        Some(t) => (t, "github.properties"),
        None => (std::env::var("GH_PACKAGES_TOKEN").unwrap_or_default(), "GH_PACKAGES_TOKEN"),
    };
    let hint = format!("ghUsername and ghAccessToken in {} (or GH_PACKAGES_USER and GH_PACKAGES_TOKEN): a classic token with read:packages", file.display());
    c.line(!user.is_empty(), true, "GitHub Packages user", &if user.is_empty() { hint.clone() } else { format!("set ({user_from})") });
    if token.is_empty() {
        c.line(false, true, "GitHub Packages token", &hint);
    } else if token.starts_with("github_pat_") {
        c.line(false, true, "GitHub Packages token", &format!("set ({token_from}), but it is a fine-grained token, which cannot read GitHub Packages: make a classic one with read:packages"));
    } else {
        let kind = if token.starts_with("ghp_") { "a classic token" } else { "not a kind this tool knows" };
        c.line(true, true, "GitHub Packages token", &format!("set ({token_from}; {kind}, {} characters; its scopes are not checked here: it needs read:packages)", token.len()));
    }
    // for install and install --release
    for (program, env, what) in [("adb", "ADB", "app install, phone"), ("gh", "GH", "app install --release")] {
        let name = std::env::var(env).ok().filter(|p| !p.is_empty()).unwrap_or_else(|| program.to_string());
        let runs = Command::new(&name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        c.line(runs, false, program, &if runs { format!("{name} runs") } else { format!("{name} does not run (needed for: {what})") });
    }
    if c.failed > 0 {
        bail!("{} of the build's prerequisites are missing", c.failed);
    }
    println!("the build's prerequisites are there (not checked: that the token can read the SESL packages)");
    Ok(())
}

// ---------------------------------------------------------------- install

/// Download the CI prerelease `oneios-<version>`'s APK with gh; returns its path.
fn download_release(version: &str, repo: &str, into: &Path) -> Result<PathBuf> {
    plain(version, "the version")?;
    plain(repo, "the repository")?;
    fs::create_dir_all(into).with_context(|| into.display().to_string())?;
    let gh = std::env::var("GH").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "gh".into());
    let name = format!("OneOS-oneios-{version}-debug.apk");
    let tag = format!("oneios-{version}");
    println!("gh release download {tag} --repo {repo} ({name})");
    let status = Command::new(&gh)
        .args(["release", "download", &tag, "--repo", repo, "--pattern", &name, "--pattern", &format!("{name}.sha256"), "--clobber", "--dir"])
        .arg(into)
        .status()
        .with_context(|| format!("cannot start {gh} (GitHub's CLI; or GH=)"))?;
    if !status.success() {
        bail!("gh release download {tag} ended with {status}");
    }
    let apk = into.join(&name);
    let (_, sha) = file_facts(&apk)?;
    match fs::read_to_string(into.join(format!("{name}.sha256"))) {
        Ok(line) if line.split_whitespace().next() == Some(sha.as_str()) => println!("sha256 as the release's .sha256: {sha}"),
        Ok(line) => bail!("{}: sha256 {sha}, the release's .sha256 says {}", apk.display(), line.split_whitespace().next().unwrap_or("nothing")),
        Err(_) => println!("sha256: {sha} (the release has no .sha256 to check it against)"),
    }
    Ok(apk)
}

pub fn install(fork: Option<&Fork>, serial: Option<&str>, apk: Option<&str>, release: Option<&str>, repo: &str, downloads: &Path) -> Result<()> {
    let apk = match (apk, release) {
        (Some(_), Some(_)) => bail!("an APK path or --release VERSION, not both"),
        (Some(path), None) => PathBuf::from(path),
        (None, Some(version)) => download_release(version, repo, downloads)?,
        (None, None) => {
            let fork = fork.ok_or_else(|| anyhow!("no APK named and no OneOS checkout whose build to take"))?;
            say_fork(fork);
            fork.dir.join(APK)
        }
    };
    let (size, sha) = file_facts(&apk)?;
    let certs = apk_certs(&apk)?;
    println!("APK: {} ({size} bytes, sha256 {sha})", apk.display());
    println!("certificate sha256: {}", certs.join(", "));
    let adb = Adb::connect(serial)?;
    adb.need_root()?;
    adb.refuse_in_session()?;
    match installed_app(&adb, APP_ID)? {
        None => println!("installed: nothing yet"),
        Some((code, name)) => {
            let have = installed_certs(&adb, APP_ID)?;
            println!("installed: {name} (versionCode {code}), certificate sha256 {}", have.join(", "));
            if have != certs {
                bail!(
                    "refused: the installed app is signed with another certificate, so this APK cannot go over it. Do not \
                     uninstall: that wipes the app's data. Install an APK signed with the installed key (the CI \
                     prerelease: --release VERSION, or a build with that key)"
                );
            }
        }
    }
    let r = adb.install(&apk.display().to_string())?;
    print!("{}", r.out);
    if !r.ok() || !r.out.lines().any(|l| l.trim() == "Success") {
        bail!("adb install -r failed: {}", last_lines(&format!("{}{}", r.out, r.err), 3));
    }
    match installed_app(&adb, APP_ID)? {
        Some((code, name)) => println!("installed now: {name} (versionCode {code})"),
        None => bail!("adb install said Success, but {APP_ID} is not listed"),
    }
    Ok(())
}

// ---------------------------------------------------------------- stage-scripts

/// Copy the fork's oneios_switch.sh, oneios_return.sh and oneios_root.sh into the stage (for sessions started
/// without the app), with each one's SHA-256 before and after.
pub fn stage_scripts(fork: &Fork, serial: Option<&str>) -> Result<()> {
    say_fork(fork);
    let mut local = Vec::new();
    for (src, dst) in SCRIPTS {
        let path = fork.dir.join(src);
        let (_, sha) = file_facts(&path)?;
        local.push((path, dst, sha));
    }
    let adb = Adb::connect(serial)?;
    adb.need_root()?;
    adb.refuse_in_session()?;
    let sums = |adb: &Adb| -> Result<Vec<String>> {
        let script: String = SCRIPTS.iter().map(|(_, dst)| format!("sha256sum {STAGE}/{dst} 2>/dev/null | cut -d' ' -f1 | grep . || echo none\n")).collect();
        Ok(adb.root_ok(&script, "reading the staged scripts' sha256")?.lines().map(str::to_string).collect())
    };
    let before = sums(&adb)?;
    let tmp = "/data/local/tmp/oneios-scripts-tmp";
    let r = adb.shell(&format!("rm -rf {tmp}; mkdir {tmp}"))?;
    if !r.ok() {
        bail!("cannot make {tmp} on the phone: {}", last_lines(&r.err, 2));
    }
    let mut script = format!("set -e; [ -d {STAGE} ] || {{ echo 'no stage: set OneiOS up in the app first'; exit 1; }}; mkdir -p {STAGE}/bin\n");
    for (i, (path, dst, _)) in local.iter().enumerate() {
        adb.push(&path.display().to_string(), &format!("{tmp}/{i}"))?;
        // a new file moved into place: what is there (a link too) is replaced, never written through
        script += &format!("cp {tmp}/{i} {STAGE}/{dst}.part && chown root:root {STAGE}/{dst}.part && chmod 0700 {STAGE}/{dst}.part && mv -f {STAGE}/{dst}.part {STAGE}/{dst}\n");
    }
    script += &format!("sync; rm -rf {tmp}\n");
    adb.root_ok(&script, "staging the scripts")?;
    let after = sums(&adb)?;
    let mut wrong = 0;
    for (i, (_, dst, sha)) in local.iter().enumerate() {
        let (was, is) = (before.get(i).map(String::as_str).unwrap_or("?"), after.get(i).map(String::as_str).unwrap_or("?"));
        let note = if is != sha {
            wrong += 1;
            "NOT the fork's file"
        } else if was == is {
            "unchanged"
        } else {
            "replaced"
        };
        println!("{dst}: before {was}, after {is} ({note})");
    }
    if wrong > 0 {
        bail!("{wrong} staged scripts are not the fork's files");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder of this test's own; removed when the test ends.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("oneios-app-lookup-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(fs::canonicalize(&dir).unwrap())
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let dir = self.0.join(rel);
            fs::create_dir_all(&dir).unwrap();
            dir
        }
        /// A folder that holds what a fork checkout holds.
        fn checkout(&self, rel: &str) -> PathBuf {
            let dir = self.dir(rel);
            for mark in MARKS {
                fs::create_dir_all(dir.join(mark).parent().unwrap()).unwrap();
                fs::write(dir.join(mark), "stand-in\n").unwrap();
            }
            dir
        }
        /// A OneiOS checkout at `oneios` that is its own main checkout, the user standing in it.
        fn places(&self) -> Places {
            let root = self.dir("oneios");
            Places { option: None, env: None, here: root.clone(), root: Some(root.clone()), main: Some(root) }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn text(p: &Path) -> String {
        p.display().to_string()
    }

    #[test]
    fn app_of_the_checkout_is_the_default_and_wins_over_the_clone_beside() {
        let s = Scratch::new("app");
        let (app, _beside) = (s.checkout("oneios/app"), s.checkout("OneOS"));
        let (fork, notes) = look(&s.places()).unwrap();
        assert_eq!((fork.dir, fork.how), (app, "app/ of the OneiOS checkout"));
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn an_empty_app_is_no_checkout_and_the_error_names_the_command() {
        let s = Scratch::new("empty");
        let app = s.dir("oneios/app");
        let e = format!("{:#}", look(&s.places()).err().expect("an empty app/ was taken for a checkout"));
        assert!(e.starts_with("no OneOS checkout"), "{e}");
        assert!(e.contains(&format!("{} is empty: the app submodule is not initialised (in {}: git submodule update --init --remote app)", text(&app), text(&s.0.join("oneios")))), "{e}");
        assert!(e.contains(&format!("{} is not there", text(&s.0.join("OneOS")))), "{e}");
    }

    #[test]
    fn an_empty_app_is_passed_over_with_a_note_when_a_later_place_is_a_checkout() {
        let s = Scratch::new("empty-beside");
        let (app, beside) = (s.dir("oneios/app"), s.checkout("OneOS"));
        let (fork, notes) = look(&s.places()).unwrap();
        assert_eq!((fork.dir, fork.how), (beside, "beside the OneiOS checkout"));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with(&format!("{} is empty: the app submodule is not initialised", text(&app))) && notes[0].contains(SUBMODULE), "{notes:?}");
    }

    #[test]
    fn a_checkout_is_told_by_what_it_holds_not_by_its_name() {
        let s = Scratch::new("holds");
        // app/ with files, but not the fork's: only the build file (upstream's branch has no oneios_switch.sh)
        let app = s.dir("oneios/app/app-android/app");
        fs::write(app.join("build.gradle.kts"), "x").unwrap();
        assert_eq!(folder(&s.0.join("oneios/app")), Folder::Lacks("switcher/assets/oneios_switch.sh"));
        fs::write(s.dir("oneios/other").join("README.md"), "x").unwrap();
        assert_eq!(folder(&s.0.join("oneios/other")), Folder::Lacks("app-android/app/build.gradle.kts"));
        assert_eq!(folder(&s.dir("oneios/nothing")), Folder::Empty);
        assert_eq!(folder(&s.0.join("oneios/absent")), Folder::NotThere);
        // a folder of any name that holds both is one
        assert_eq!(folder(&s.checkout("somewhere/else")), Folder::Checkout);
        let e = format!("{:#}", look(&s.places()).err().unwrap());
        assert!(e.contains("oneios/app is not the fork's checkout (no switcher/assets/oneios_switch.sh; in "), "{e}");
        // and it is passed over, with a note, for the clone beside
        let beside = s.checkout("OneOS");
        let (fork, notes) = look(&s.places()).unwrap();
        assert_eq!(fork.dir, beside);
        assert!(notes.len() == 1 && notes[0].contains("is not the fork's checkout"), "{notes:?}");
    }

    #[test]
    fn a_checkout_named_by_hand_wins_over_app() {
        let s = Scratch::new("named");
        let (app, mine, other) = (s.checkout("oneios/app"), s.checkout("mine"), s.checkout("other"));
        let (fork, notes) = look(&Places { option: Some(text(&mine)), ..s.places() }).unwrap();
        assert_eq!((fork.dir, fork.how), (mine.clone(), "--oneos"));
        assert!(notes.is_empty(), "{notes:?}");
        // ONEOS_DIR too, and it says that the submodule is not used
        let (fork, notes) = look(&Places { env: Some(text(&other)), ..s.places() }).unwrap();
        assert_eq!((fork.dir, fork.how), (other.clone(), "ONEOS_DIR"));
        assert_eq!(notes, vec![format!("ONEOS_DIR is set, so {} is not used", text(&app))]);
        // ONEOS_DIR that names app/ itself: nothing to say
        let (fork, notes) = look(&Places { env: Some(text(&app)), ..s.places() }).unwrap();
        assert_eq!((fork.dir, fork.how, notes.len()), (app, "ONEOS_DIR", 0));
        // --oneos before ONEOS_DIR
        let (fork, _) = look(&Places { option: Some(text(&mine)), env: Some(text(&other)), ..s.places() }).unwrap();
        assert_eq!((fork.dir, fork.how), (mine, "--oneos"));
    }

    #[test]
    fn a_named_folder_that_is_no_checkout_is_an_error_not_a_reason_to_look_further() {
        let s = Scratch::new("named-wrong");
        let (_app, empty) = (s.checkout("oneios/app"), s.dir("empty"));
        fs::write(s.dir("files").join("x"), "x").unwrap();
        let (files, absent) = (s.0.join("files"), s.0.join("absent"));
        for (places, want) in [
            (Places { option: Some(text(&empty)), ..s.places() }, format!("--oneos: {} is an empty folder, not a OneOS checkout (if it is OneiOS's app/, the submodule is not initialised: {SUBMODULE})", text(&empty))),
            (Places { env: Some(text(&empty)), ..s.places() }, format!("ONEOS_DIR: {} is an empty folder", text(&empty))),
            (Places { env: Some(text(&files)), ..s.places() }, format!("ONEOS_DIR: {} is not a OneOS checkout (no app-android/app/build.gradle.kts)", text(&files))),
            (Places { option: Some(text(&absent)), ..s.places() }, format!("--oneos: {}", text(&absent))),
        ] {
            let e = format!("{:#}", look(&places).err().expect("a folder that is no checkout was taken, or the lookup went on"));
            assert!(e.starts_with(&want), "{e}\n wanted: {want}");
        }
    }

    #[test]
    fn the_current_folder_when_it_is_a_checkout_comes_before_app() {
        let s = Scratch::new("here");
        let (_app, here) = (s.checkout("oneios/app"), s.checkout("standing"));
        let (fork, _) = look(&Places { here: here.clone(), ..s.places() }).unwrap();
        assert_eq!((fork.dir, fork.how), (here, "the current folder"));
    }

    #[test]
    fn from_a_worktree_the_main_checkout_app_and_then_the_clone_beside_it() {
        let s = Scratch::new("worktree");
        let (main, tree) = (s.dir("oneios"), s.dir("oneios/.claude/worktrees/agent-x"));
        let places = || Places { option: None, env: None, here: tree.clone(), root: Some(tree.clone()), main: Some(main.clone()) };
        // the worktree has no app/ (the submodule is not on its branch yet): the clone beside the main checkout
        let beside = s.checkout("OneOS");
        let (fork, notes) = look(&places()).unwrap();
        assert_eq!((fork.dir, fork.how, notes.len()), (beside, "beside the OneiOS checkout", 0));
        // a new worktree's app/ is empty; the main checkout's is initialised
        let (empty, app) = (s.dir("oneios/.claude/worktrees/agent-x/app"), s.checkout("oneios/app"));
        let (fork, notes) = look(&places()).unwrap();
        assert_eq!((fork.dir, fork.how), (app, "app/ of the OneiOS main checkout"));
        assert!(notes.len() == 1 && notes[0].starts_with(&format!("{} is empty", text(&empty))), "{notes:?}");
        // the worktree's own app/, once initialised, is first
        let own = s.checkout("oneios/.claude/worktrees/agent-x/app");
        assert_eq!(look(&places()).unwrap().0.dir, own);
    }

    #[test]
    fn none_found() {
        let s = Scratch::new("none");
        let e = format!("{:#}", look(&s.places()).err().unwrap());
        for want in [
            "no OneOS checkout (a folder with app-android/app/build.gradle.kts and switcher/assets/oneios_switch.sh): ".to_string(),
            format!("{} is not there; {} is not there", text(&s.0.join("oneios/app")), text(&s.0.join("OneOS"))),
            format!("Name one with --oneos DIR or ONEOS_DIR, or get the submodule (in the OneiOS checkout: {SUBMODULE})"),
        ] {
            assert!(e.contains(&want), "{e}\n wanted: {want}");
        }
        // outside every checkout
        let e = format!("{:#}", look(&Places { option: None, env: None, here: s.dir("x"), root: None, main: None }).err().unwrap());
        assert!(e.contains("the current folder is not inside a OneiOS checkout"), "{e}");
    }
}
