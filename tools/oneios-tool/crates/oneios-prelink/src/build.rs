//! `prelink build`: the steps of kernel/prelink/build.sh in one process, for the phone (which has no bash and
//! starts no other program) and for `oneios-tool serve`. The same steps with the same arguments, each through the
//! step's own command line, so `OUT/work` and `OUT` hold the files build.sh leaves, byte for byte
//! (tools/oneios-tool/README.md "prelink": compared on the real inputs). Added to build.sh's steps: `stubs-check`
//! on the collection before it is put in place.
//!
//! Where build.sh runs one step after the other, steps that wait for nothing of each other run at the same time
//! here, in the pool's threads: `native-input` and `merge-reviewed` beside `roots` and `hosted`; the two `align`
//! beside `kclasses` and `structors`; `audit`, `stubs-check`, `record` and `hashes`. `match`, `vtables` and `emit`
//! run alone. Every step still reads and writes its own files.
//!
//! Not here: build.sh's `X18=1` (the hosted step would start llvm-mc).
//!
//! Between steps, and between the kexts of the hosted and the emit step, the running request's stop is asked
//! (`oneios_common::job::check`: a cancel, the heat and battery limits of `serve`); a build that runs alone is
//! never stopped.

use anyhow::{anyhow, bail, Context, Result};
use oneios_common::job;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    /// The hosted kernel, xnu.macho (kernel/build-hosted.sh, or the released one)
    #[arg(long)]
    pub kernel: String,
    /// The decoded kernelcache (kernelcache.macho: `image im4p` of the IPSW's kernelcache)
    #[arg(long)]
    pub kernelcache: String,
    /// The kernel profile (profile.toml: its [kernel.prelink] pins the kernelcache and names the roots)
    #[arg(long)]
    pub profile: String,
    /// The owner's reviewed records (kernel/prelink/reviewed-BUILD-vNN.json)
    #[arg(long)]
    pub reviewed: String,
    /// More reviewed records merged into them (kernel/prelink/lead/reviewed-*.json, user/reviewed-*.json), in order
    #[arg(long = "more-reviewed", value_name = "FILE")]
    pub more_reviewed: Vec<String>,
    /// The known-good pairs (kernel/prelink/pairs-BUILD.json)
    #[arg(long)]
    pub pairs: String,
    /// The stub list (kernel/prelink/stubs-BUILD.json)
    #[arg(long)]
    pub stubs: String,
    /// The XnuPrelink checkout: its reviewed fixtures and the files the matcher signature is made of
    #[arg(long)]
    pub xnuprelink: String,
    /// The RELEASE kernel of the same source with symbols (build.sh's REFERENCE), optional
    #[arg(long)]
    pub reference: Option<String>,
    /// Where kernel.kc and its files go; the steps' own files go to OUT/work
    #[arg(long)]
    pub out: String,
    /// The OneiOS checkout, for the record (default: the folder the reviewed records' folder is two levels below)
    #[arg(long)]
    pub repo: Option<String>,
    /// Never start `git` for the record's two commits (the phone, `serve`): they are the two options below, or null
    #[arg(long)]
    pub no_git: bool,
    #[arg(long)]
    pub oneios_commit: Option<String>,
    #[arg(long)]
    pub xnuprelink_commit: Option<String>,
    /// Remove OUT/work when the build is done (it holds copies of the kexts' code)
    #[arg(long)]
    pub remove_work: bool,
}

/// The steps, in the order they run, each with a line for a screen.
pub const STEPS: [(&str, &str); 16] = [
    ("check", "Checking the inputs"),
    ("native-input", "Preparing the kernel"),
    ("merge-reviewed", "Merging the reviewed records"),
    ("roots", "Choosing the kexts"),
    ("hosted", "Rewriting the kexts for the hosted kernel"),
    ("match", "Pairing the original kernel with ours"),
    ("align", "Pairing by link order"),
    ("kclasses", "Pairing the kernel's classes"),
    ("structors", "Pairing constructors and destructors"),
    ("vtables", "Pairing virtual methods"),
    ("emit", "Writing the kernel collection"),
    ("audit", "Checking every import's binding"),
    ("stubs-check", "Checking the stubs"),
    ("record", "Writing the build record"),
    ("hashes", "Hashing the kernel collection"),
    ("place", "Putting the files in place"),
];

/// Why a build was refused (not a failure of a step: the inputs or the result are not the ones wanted).
#[derive(Debug)]
pub enum Refused {
    /// A file the build needs is not there.
    NotFound(String),
    /// The kernelcache is not the one the profile pins.
    Mismatch(String),
    /// The audit or the stub check found a wrong binding; kernel.kc was not put in place.
    Audit(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotFound(m) | Refused::Mismatch(m) | Refused::Audit(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Refused {}

/// What a build made.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub kc: String,
    pub bytes: u64,
    pub sha256: String,
}

fn sha(path: &str) -> Result<String> {
    Ok(oneios_common::sha256_hex(&oneios_common::Map::open(path)?))
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// Two parts of the build at the same time, each in a thread of the pool; the first one's error when both fail (a
/// part that failed does not end the other: it runs to its end).
fn both<A: Send, B: Send>(a: impl FnOnce() -> Result<A> + Send, b: impl FnOnce() -> Result<B> + Send) -> Result<(A, B)> {
    let (a, b) = rayon::join(a, b);
    Ok((a?, b?))
}

/// One step through its own command line, as build.sh's `step` runs it. A step that ends with another status than 0
/// has said why on stderr.
fn step(name: &str, words: Vec<String>) -> Result<()> {
    job::check()?;
    let mut line = vec![name.to_string()];
    line.extend(words);
    match crate::run(&line) {
        0 => Ok(()),
        // the step's own stop (a cancel, the heat limit) is the reason, not its status
        _ if job::stop_reason().is_some() => Err(job::stop_reason().expect("a stop").into()),
        status => bail!("prelink {name} failed (status {status}): its own lines say why"),
    }
}

/// `--pin`'s value: the kernelcache SHA-256 the profile pins.
fn pinned(profile: &str) -> Result<String> {
    let table: toml::Table = std::fs::read_to_string(profile).with_context(|| profile.to_string())?.parse().with_context(|| profile.to_string())?;
    let pin = table.get("kernel").and_then(|k| k.get("prelink")).and_then(|p| p.get("kernelcache_sha256")).and_then(|v| v.as_str());
    Ok(pin.ok_or_else(|| anyhow!("{profile}: no [kernel.prelink] kernelcache_sha256"))?.to_string())
}

/// The whole build. `OUT/work` is made when it is not there; `kernel.kc`, `kernel.kc.hashes`, `kernel.kc.build.json`,
/// `kernel.kc.prelink.json` and `kernel.kc.binds.json` are put into `OUT` only after the audit and the stub check
/// passed.
pub fn build(args: &Args) -> Result<Built> {
    job::step("check");
    let fixture = format!("{}/fileset_match.py", args.xnuprelink);
    let mut needed = vec![&args.kernel, &args.kernelcache, &args.profile, &args.reviewed, &args.pairs, &args.stubs, &fixture];
    needed.extend(args.more_reviewed.iter());
    needed.extend(args.reference.iter());
    for file in needed {
        if !Path::new(file).is_file() {
            return Err(Refused::NotFound(format!("{file} is missing")).into());
        }
    }
    let want = pinned(&args.profile)?;
    if sha(&args.kernelcache)? != want {
        return Err(Refused::Mismatch(format!("{} is not the kernelcache the profile pins ({want})", args.kernelcache)).into());
    }
    let out = PathBuf::from(&args.out);
    let work = out.join("work");
    std::fs::create_dir_all(&work).with_context(|| work.display().to_string())?;
    // (--remove-work: gone when the build ends, however it ends; without it work stays, as build.sh leaves it, with
    // its pairing to reuse and the audit's list to read)
    let work_guard = args.remove_work.then(|| job::temp(&work));
    let w = |name: &str| work.join(name).to_string_lossy().into_owned();
    let x = args.xnuprelink.as_str();
    let cache = args.kernelcache.as_str();

    // Steps that wait for nothing of each other run at the same time, in the pool's threads (`both`): each is its
    // own command with its own files, as when build.sh runs them one after the other, so the files are the same.
    // Their one-line reports may come in another order than build.sh's.

    // 1. The kernel as the emitter's native input (its one weak bind resolved), the reference the same way, and the
    //    other agents' reviewed records merged into ours; beside them 2. which kexts, and 3. the kexts for the
    //    hosted kernel.
    let reviewed = w("reviewed.merged.json");
    let (reference, input) = both(
        || -> Result<Option<String>> {
            job::step("native-input");
            std::fs::copy(&args.kernel, w("xnu.macho")).with_context(|| args.kernel.clone())?;
            step("native-input", vec![w("xnu.macho"), w("native.macho"), w("native-input.json")])?;
            let reference = match &args.reference {
                Some(reference) => {
                    step("native-input", vec![reference.clone(), w("reference.macho"), w("reference-input.json")])?;
                    Some(w("reference.macho"))
                }
                None => None,
            };
            job::step("merge-reviewed");
            let mut line = vec!["--reviewed".to_string(), args.reviewed.clone(), "--native".into(), w("native.macho"), "--out".into(), reviewed.clone()];
            line.extend(args.more_reviewed.iter().cloned());
            step("merge-reviewed", line)?;
            Ok(reference)
        },
        || -> Result<String> {
            job::step("roots");
            step("roots", vec![cache.to_string(), w("select.json"), args.profile.clone()])?;
            job::step("hosted");
            let hosted = w("kernelcache.hosted.macho");
            let _ = std::fs::remove_file(&hosted);
            step("hosted", vec![cache.to_string(), hosted.clone(), w("hosted.json"), "--closure".into(), w("select.json")])?;
            Ok(if Path::new(&hosted).is_file() { hosted } else { cache.to_string() })
        },
    )?;
    // 4. The pairing, reused while both inputs, the reference, this program and XnuPrelink's matcher are the same.
    job::step("match");
    let tool = std::env::current_exe().context("this program's own path")?.to_string_lossy().into_owned();
    let signature = crate::matcher::matcher_signature(Path::new(x))?;
    let key = match &reference {
        Some(reference) => format!("{} {} {} {} {signature}", sha(&input)?, sha(&w("native.macho"))?, sha(reference)?, sha(&tool)?),
        None => format!("{} {}  {} {signature}", sha(&input)?, sha(&w("native.macho"))?, sha(&tool)?),
    };
    let same = Path::new(&w("match.map.json")).is_file() && std::fs::read_to_string(w("match.key")).is_ok_and(|k| k.trim_end_matches('\n') == key);
    if !same {
        let mut line = vec![input.clone(), w("native.macho"), "--out".into(), w("match.map.json")];
        if let Some(reference) = &reference {
            line.extend(["--reference-kernel".to_string(), reference.clone()]);
        }
        line.extend(["--xnuprelink".to_string(), x.to_string()]);
        step("match", line)?;
        std::fs::write(w("match.key"), format!("{key}\n")).with_context(|| w("match.key"))?;
    }
    // 5. What the matcher left unpaired, by link order (the kernel's own, then the reference's); beside it the
    //    kernel's class objects and then the constructors and destructors; after both, the virtual methods.
    let common = |extra: &[&str]| -> Vec<String> {
        let mut line = strings(&["--xnuprelink", x, "--kernelcache", &input, "--native", &w("native.macho"), "--map", &w("match.map.json")]);
        line.extend(strings(extra));
        line
    };
    let (aligned, ()) = both(
        || -> Result<Vec<String>> {
            job::step("align");
            let mut line = common(&["--reference", &w("native.macho")]);
            if let Some(reference) = &reference {
                line.extend(strings(&["--features", reference]));
            }
            line.extend(strings(&["--also", &reviewed, "--out", &w("aligned-native.json")]));
            step("align", line)?;
            let mut aligned = vec![w("aligned-native.json")];
            match &reference {
                Some(reference) => {
                    step("align", common(&["--reference", reference, "--position", "--also", &reviewed, "--also", &w("aligned-native.json"), "--out", &w("aligned-reference.json")]))?;
                    aligned.push(w("aligned-reference.json"));
                }
                None => {
                    let _ = std::fs::remove_file(w("aligned-reference.json"));
                }
            }
            Ok(aligned)
        },
        || -> Result<()> {
            job::step("kclasses");
            step("kclasses", common(&["--reviewed", &reviewed, "--out", &w("kclasses.json")]))?;
            job::step("structors");
            step("structors", common(&["--reviewed", &reviewed, "--classes", &w("kclasses.json"), "--out", &w("structors.json")]))
        },
    )?;
    job::step("vtables");
    let mut line = common(&["--reviewed", &reviewed, "--classes", &w("kclasses.json"), "--also", &w("structors.json")]);
    for file in &aligned {
        line.extend(strings(&["--weak", file]));
    }
    line.extend(strings(&["--out", &w("vtables.json")]));
    step("vtables", line)?;
    // 6. The collection: the emit step takes the first pair it is given for a function.
    job::step("emit");
    let mut line = strings(&["--xnuprelink", x, "--kernelcache", &input, "--original-kernelcache", cache, "--native", &w("native.macho"), "--map", &w("match.map.json"), "--roots", &w("select.json"), "--reviewed", &reviewed]);
    for file in [w("kclasses.json"), w("vtables.json"), w("structors.json")].iter().chain(&aligned) {
        line.extend(strings(&["--aligned", file]));
    }
    line.extend(strings(&["--pairs", &args.pairs, "--stubs", &args.stubs, "--output", &w("kernel.kc"), "--report", &w("kernel.kc.prelink.json")]));
    step("emit", line)?;
    // 6b. Every kext import of the collection, and every stub of the kernel: a failure stops the build before
    //     kernel.kc is put in place. Beside them 7. the record and 8. the collection's chunk hash list
    //     (docs/image-format.md: SHA-256 of each 1 MiB chunk), which read the collection and change nothing of it.
    let hashes = job::temp(out.join("kernel.kc.hashes.partial"));
    let (((), ()), ((), (bytes, sha256))) = both(
        || {
            both(
                || -> Result<()> {
                    job::step("audit");
                    job::check()?;
                    let line = strings(&["audit", "--xnuprelink", x, "--kernelcache", cache, "--collection", &w("kernel.kc"), "--pairs", &args.pairs, "--stubs", &args.stubs, "--reviewed", &reviewed, "--report", &w("kernel.kc.binds.json")]);
                    match crate::parse(&line)? {
                        crate::Command::Audit(audit) => {
                            if crate::audit::run(audit).context("prelink audit")? {
                                let list = if args.remove_work { "the lines before this name the first ones".to_string() } else { format!("{} lists every import", w("kernel.kc.binds.json")) };
                                return Err(Refused::Audit(format!("the audit found an import off the known-good pairs or on the stub without a reason ({list}). kernel.kc was not put in place")).into());
                            }
                            Ok(())
                        }
                        _ => bail!("the audit's command line did not parse as the audit"),
                    }
                },
                || -> Result<()> {
                    job::step("stubs-check");
                    job::check()?;
                    if crate::stubs_check::main(&w("kernel.kc"))? != 0 {
                        return Err(Refused::Audit("a stub of the kernel does not branch to its .original symbol (the lines before this name it). kernel.kc was not put in place".into()).into());
                    }
                    Ok(())
                },
            )
        },
        || {
            both(
                || -> Result<()> {
                    job::step("record");
                    let repo = match &args.repo {
                        Some(repo) => repo.clone(),
                        // kernel/prelink/reviewed-*.json: the checkout is the folder above kernel/
                        None => Path::new(&args.reviewed).parent().and_then(Path::parent).and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into()),
                    };
                    let mut line = strings(&["--work", &work.to_string_lossy(), "--kernel", &args.kernel, "--kernelcache", cache, "--reviewed", &reviewed, "--xnuprelink", x]);
                    if let Some(reference) = &reference {
                        line.extend(strings(&["--reference", reference]));
                    }
                    line.extend(strings(&["--output", &w("kernel.kc.build.json"), "--repo", &repo]));
                    if args.no_git {
                        line.push("--no-git".into());
                    }
                    for (option, value) in [("--oneios-commit", &args.oneios_commit), ("--xnuprelink-commit", &args.xnuprelink_commit)] {
                        if let Some(value) = value {
                            line.extend(strings(&[option, value]));
                        }
                    }
                    step("record", line)
                },
                || -> Result<(u64, String)> {
                    job::step("hashes");
                    job::check()?;
                    oneios_image::hashes_file(&w("kernel.kc"), &hashes.path().to_string_lossy(), 1 << 20).map_err(|e| anyhow!("{e}"))
                },
            )
        },
    )?;
    job::step("place");
    job::check()?;
    let partial = job::temp(out.join("kernel.kc.partial"));
    std::fs::copy(w("kernel.kc"), partial.path()).with_context(|| partial.path().display().to_string())?;
    for name in ["kernel.kc.prelink.json", "kernel.kc.build.json", "kernel.kc.binds.json"] {
        std::fs::copy(w(name), out.join(name)).with_context(|| out.join(name).display().to_string())?;
    }
    let kc = out.join("kernel.kc");
    std::fs::rename(partial.path(), &kc).with_context(|| kc.display().to_string())?;
    partial.keep();
    std::fs::rename(hashes.path(), out.join("kernel.kc.hashes")).with_context(|| out.join("kernel.kc.hashes").display().to_string())?;
    hashes.keep();
    drop(work_guard);
    Ok(Built { kc: kc.to_string_lossy().into_owned(), bytes, sha256 })
}

/// `oneios-tool prelink build ...`: build.sh's last line.
pub fn run(args: Args) -> Result<()> {
    let built = build(&args)?;
    println!("prelink: {} ({} bytes, sha256 {}...)", built.kc, built.bytes, &built.sha256[..16]);
    Ok(())
}
