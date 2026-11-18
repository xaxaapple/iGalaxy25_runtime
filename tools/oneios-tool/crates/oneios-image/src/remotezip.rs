//! `oneios-tool remotezip` (was scripts/fetch/remotezip.py): read single members of a ZIP (an IPSW) without
//! downloading the archive. `image check-ipsw` and `image fetch` use it.
//!
//!   remotezip ls  SOURCE [REGEX]                    size, packed size, method, CRC-32, name of each member
//!   remotezip cat SOURCE MEMBER                     a small member on stdout
//!   remotezip get SOURCE OUTDIR [-j N] MEMBER[=NAME]...   members into OUTDIR (NAME: file name there)
//!   remotezip head SOURCE MEMBER BYTES              the first BYTES of a stored member on stdout
//!
//! SOURCE is an https URL (HTTP range requests) or a local file. `get` fetches a member's packed bytes in
//! parallel pieces into NAME.part; the byte ranges that are there are listed in NAME.part.state, so an
//! interrupted run resumes. Deflated members are inflated locally. The CRC-32 of the ZIP directory is checked
//! before the file gets its final name. Linux and macOS.
//!
//! The requests themselves (the connections and their addresses, the tries after a drop and their waits, the
//! pieces, the state file) are transfer.rs.
//!
//! Same idea as iOSPlus10's `op10tool ipsw-get` and UniXNU's scripts/unixnu/ipsw.py.
//!
//! Libraries: ureq (HTTP, in transfer.rs), flate2 (inflate), crc32fast, regex.
//! The ZIP directory is read here (70 lines: the end record, its ZIP64 form, the entries): the reader needs the
//! directory alone, from two range requests, and each member's own header only when that member is read.

pub use crate::transfer::{download, Fetched, Frontier, Result, Source, CHUNK};
use crate::transfer::Part;
use oneios_common::job;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

/// One member of the archive, as its directory has it.
#[derive(Clone, Debug)]
pub struct Info {
    pub filename: String,
    pub file_size: u64,
    pub compress_size: u64,
    pub crc: u32,
    /// 0 stored, 8 deflate.
    pub compress_type: u16,
    pub flag_bits: u16,
    pub header_offset: u64,
}

fn u16_at(data: &[u8], at: usize) -> u64 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as u64
}

fn u32_at(data: &[u8], at: usize) -> u64 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as u64
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

/// The archive's directory: where it is (the end record, or its ZIP64 form) and its entries.
fn directory(source: &Source) -> Result<Vec<Info>> {
    let not_zip = || format!("{}: not a ZIP archive", source.spec);
    // the end record is the last 22 bytes, or before a comment of up to 64 KiB; the ZIP64 locator is 20 bytes before it
    let tail_start = source.size.saturating_sub(22 + 65535 + 20);
    let tail = source.read(tail_start, source.size.saturating_sub(1))?;
    let end = (0..tail.len().saturating_sub(21)).rev().find(|&at| &tail[at..at + 4] == b"PK\x05\x06").ok_or_else(not_zip)?;
    let (mut count, mut size, mut offset) = (u16_at(&tail, end + 10), u32_at(&tail, end + 12), u32_at(&tail, end + 16));
    if end >= 20 && &tail[end - 20..end - 16] == b"PK\x06\x07" {
        let record = source.read(u64_at(&tail, end - 12), u64_at(&tail, end - 12) + 55)?;
        if &record[..4] != b"PK\x06\x06" {
            return Err(not_zip());
        }
        (count, size, offset) = (u64_at(&record, 32), u64_at(&record, 40), u64_at(&record, 48));
    }
    let data = source.read(offset, (offset + size).saturating_sub(1))?;
    let (mut infos, mut at) = (Vec::new(), 0usize);
    for _ in 0..count {
        if at + 46 > data.len() || &data[at..at + 4] != b"PK\x01\x02" {
            return Err(not_zip());
        }
        let (name_len, extra_len, comment_len) = (u16_at(&data, at + 28) as usize, u16_at(&data, at + 30) as usize, u16_at(&data, at + 32) as usize);
        let mut info = Info {
            filename: String::from_utf8_lossy(&data[at + 46..at + 46 + name_len]).into_owned(),
            file_size: u32_at(&data, at + 24),
            compress_size: u32_at(&data, at + 20),
            crc: u32_at(&data, at + 16) as u32,
            compress_type: u16_at(&data, at + 10) as u16,
            flag_bits: u16_at(&data, at + 8) as u16,
            header_offset: u32_at(&data, at + 42),
        };
        zip64(&mut info, &data[at + 46 + name_len..at + 46 + name_len + extra_len]);
        infos.push(info);
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(infos)
}

/// The ZIP64 extra field (id 1): the 64-bit value of each field that is all ones in the entry, in this order.
fn zip64(info: &mut Info, extra: &[u8]) {
    let mut at = 0;
    while at + 4 <= extra.len() {
        let (id, len) = (u16_at(extra, at), u16_at(extra, at + 2) as usize);
        if id == 1 {
            let mut field = at + 4;
            for value in [&mut info.file_size, &mut info.compress_size, &mut info.header_offset] {
                if *value == 0xffff_ffff && field + 8 <= at + 4 + len {
                    *value = u64_at(extra, field);
                    field += 8;
                }
            }
        }
        at += 4 + len;
    }
}

pub struct RemoteZip {
    pub source: Source,
    infos: Vec<Info>,
    /// A fetch that ends early leaves its `.part` files for the next run (true: as the command always did).
    /// `serve` sets false for a request without `"resume"`: the parts go when the request is stopped or fails.
    pub keep_parts: bool,
}

impl RemoteZip {
    pub fn open(spec: &str) -> Result<RemoteZip> {
        let source = Source::open(spec)?;
        let infos = directory(&source)?;
        Ok(RemoteZip { source, infos, keep_parts: true })
    }

    pub fn infos(&self) -> &[Info] {
        &self.infos
    }

    /// The entry called name, or the only one whose path ends in /name.
    pub fn find(&self, name: &str) -> Result<&Info> {
        if let Some(info) = self.infos.iter().rev().find(|i| i.filename == name) {
            return Ok(info);
        }
        let tail = format!("/{name}");
        let hits: Vec<&Info> = self.infos.iter().filter(|i| i.filename.ends_with(&tail)).collect();
        match hits[..] {
            [one] => Ok(one),
            [] => Err(format!("{name}: not in the archive")),
            _ => Err(format!("{name}: {} members match", hits.len())),
        }
    }

    /// Where a member's bytes begin: after its own header, whose name and extra field have their own lengths.
    fn data_start(&self, info: &Info) -> Result<u64> {
        let head = self.source.read(info.header_offset, info.header_offset + 29)?;
        if &head[..4] != b"PK\x03\x04" {
            return Err(format!("{}: no local header", info.filename));
        }
        Ok(info.header_offset + 30 + u16_at(&head, 26) + u16_at(&head, 28))
    }

    /// A whole (small) member.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let info = self.find(name)?;
        let start = self.data_start(info)?;
        let packed = self.source.read(start, (start + info.compress_size).wrapping_sub(1))?;
        let data = match info.compress_type {
            0 => packed,
            8 => {
                let mut data = Vec::with_capacity(info.file_size as usize);
                flate2::read::DeflateDecoder::new(&packed[..]).read_to_end(&mut data).map_err(|e| format!("{}: {e}", info.filename))?;
                data
            }
            other => return Err(format!("{}: compression method {other} is not supported", info.filename)),
        };
        if crc32fast::hash(&data) != info.crc {
            return Err(format!("{}: Bad CRC-32", info.filename));
        }
        Ok(data)
    }

    /// The first `count` bytes of a stored member.
    pub fn head(&self, name: &str, count: u64) -> Result<Vec<u8>> {
        let info = self.find(name)?;
        if info.compress_type != 0 {
            return Err(format!("{}: not stored, cannot read its first bytes alone", info.filename));
        }
        let start = self.data_start(info)?;
        self.source.read(start, (start + count.min(info.file_size)).wrapping_sub(1))
    }

    /// Fetch one member to the path out, `chunk` bytes at a time. Returns (size, crc32).
    pub fn get(&self, name: &str, out: &str, jobs: usize, chunk: u64, log: &(dyn Fn(String) + Sync)) -> Result<(u64, u32)> {
        let info = self.find(name)?;
        if info.compress_type != 0 && info.compress_type != 8 {
            return Err(format!("{}: compression method {} is not supported", info.filename, info.compress_type));
        }
        if info.flag_bits & 1 != 0 {
            return Err(format!("{}: encrypted ZIP member", info.filename));
        }
        if std::fs::metadata(out).is_ok_and(|m| m.is_file() && m.len() == info.file_size) && crc_file(out)? == info.crc {
            job::count(info.compress_size);
            log(format!("{}: already complete", info.filename));
            return Ok((info.file_size, info.crc));
        }
        let stored = info.compress_type == 0;
        let part = format!("{out}{}", if stored { ".part" } else { ".deflate.part" });
        let state_path = format!("{part}.state");
        let parts: Vec<job::Temp> = if self.keep_parts { Vec::new() } else { vec![job::temp(&part), job::temp(&state_path), job::temp(format!("{state_path}.tmp")), job::temp(format!("{out}.part"))] };
        if let Err(e) = self.packed(info, &part, jobs, chunk, log) {
            // (a request that was paused when the process ended keeps its part: the user had it on hold)
            if job::held_at_the_end() {
                parts.into_iter().for_each(job::Temp::keep);
            }
            return Err(format!("{}: download failed, run again to resume ({e})", info.filename));
        }
        let (tmp, crc) = if stored {
            (part.clone(), crc_file(&part)?)
        } else {
            log(format!("{}: inflating", info.filename));
            let tmp = format!("{out}.part");
            let crc = inflate(&part, &tmp).map_err(|e| format!("{}: {e}", info.filename))?;
            (tmp, crc)
        };
        let size = std::fs::metadata(&tmp).map_err(|e| format!("{tmp}: {e}"))?.len();
        if crc != info.crc || size != info.file_size {
            let _ = std::fs::remove_file(&state_path);
            return Err(format!("{}: CRC-32 {crc:08x}, the archive says {:08x}; chunk list reset, run again", info.filename, info.crc));
        }
        std::fs::rename(&tmp, out).map_err(|e| format!("{out}: {e}"))?;
        let _ = std::fs::remove_file(&state_path);
        if !stored {
            let _ = std::fs::remove_file(&part);
        }
        // (the parts are the file now, or gone)
        parts.into_iter().for_each(job::Temp::keep);
        log(format!("{}: {} bytes, CRC-32 ok -> {out}", info.filename, info.file_size));
        Ok((info.file_size, info.crc))
    }

    /// The member's packed bytes into `part`, in pieces of `chunk` bytes, `jobs` connections at a time. The part's
    /// state file (`PART.state`, transfer.rs) lists what is there and names the member, so a later run fetches the
    /// rest only.
    fn packed(&self, info: &Info, part: &str, jobs: usize, chunk: u64, log: &(dyn Fn(String) + Sync)) -> Result<Fetched> {
        let what = format!("member {:08x} {}", info.crc, info.filename);
        let part = Part { name: &info.filename, what, offset: self.data_start(info)?, length: info.compress_size, path: part, connections: jobs, piece: chunk, whole: false, frontier: None };
        self.source.fetch(&part, log)
    }
}

/// Inflate a raw deflate stream from `part` into `tmp`; the CRC-32 of what came out.
fn inflate(part: &str, tmp: &str) -> std::io::Result<u32> {
    let mut source = flate2::read::DeflateDecoder::new(std::io::BufReader::with_capacity(1 << 20, File::open(part)?));
    let mut out = std::io::BufWriter::with_capacity(1 << 20, File::create(tmp)?);
    let (mut crc, mut block) = (crc32fast::Hasher::new(), vec![0u8; 1 << 20]);
    loop {
        let n = source.read(&mut block)?;
        if n == 0 {
            out.flush()?;
            return Ok(crc.finalize());
        }
        crc.update(&block[..n]);
        out.write_all(&block[..n])?;
    }
}

pub fn crc_file(path: &str) -> Result<u32> {
    let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let (mut crc, mut block) = (crc32fast::Hasher::new(), vec![0u8; 1 << 20]);
    loop {
        let n = file.read(&mut block).map_err(|e| format!("{path}: {e}"))?;
        if n == 0 {
            return Ok(crc.finalize());
        }
        crc.update(&block[..n]);
    }
}

const USAGE: &str = "usage: oneios-tool remotezip ls SOURCE [REGEX] | cat SOURCE MEMBER | get SOURCE OUTDIR [-j N] MEMBER[=NAME]... | head SOURCE MEMBER BYTES\n(SOURCE: an https URL or a local file)";

fn command(args: &[String]) -> Result<bool> {
    let (Some(cmd), Some(spec)) = (args.first(), args.get(1)) else { return Ok(false) };
    let rest = &args[2..];
    let z = RemoteZip::open(spec)?;
    let mut out = std::io::stdout().lock();
    match (cmd.as_str(), rest) {
        ("ls", _) => {
            let pattern = rest.first().map(|p| regex::Regex::new(p).map_err(|e| e.to_string())).transpose()?;
            for i in z.infos().iter().filter(|i| pattern.as_ref().is_none_or(|p| p.is_match(&i.filename))) {
                let method = match i.compress_type {
                    0 => "stored".to_string(),
                    8 => "deflate".to_string(),
                    other => other.to_string(),
                };
                let _ = writeln!(out, "{:>13} {:>13} {method:<7} {:08x} {}", i.file_size, i.compress_size, i.crc, i.filename);
            }
        }
        ("cat", [member, ..]) => out.write_all(&z.read(member)?).map_err(|e| e.to_string())?,
        ("head", [member, count, ..]) => out.write_all(&z.head(member, count.parse().map_err(|_| format!("{count} is not a number of bytes"))?)?).map_err(|e| e.to_string())?,
        ("get", [outdir, members @ ..]) => {
            drop(out);
            // (8 requests at a time; `oneios-tool --jobs N remotezip get` names another count, as -j N does)
            let (mut jobs, mut names, mut words) = (if job::jobs_given() { job::jobs() } else { 8 }, Vec::new(), members.iter());
            while let Some(word) = words.next() {
                match word.as_str() {
                    "-j" => jobs = words.next().and_then(|n| n.parse().ok()).ok_or("-j needs a number")?,
                    _ => names.push(word),
                }
            }
            std::fs::create_dir_all(outdir).map_err(|e| format!("{outdir}: {e}"))?;
            for member in names {
                let (name, as_name) = member.split_once('=').unwrap_or((member, ""));
                let file = if as_name.is_empty() { name.rsplit('/').next().unwrap_or(name) } else { as_name };
                z.get(name, &Path::new(outdir).join(file).to_string_lossy(), jobs, CHUNK, &|line| println!("{line}"))?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `oneios-tool remotezip ARGS...`; the exit status.
pub fn main(args: &[String]) -> i32 {
    match command(args) {
        Ok(true) => 0,
        Ok(false) => {
            eprintln!("{USAGE}");
            1
        }
        Err(e) => {
            eprintln!("remotezip: {e}");
            1
        }
    }
}

#[cfg(test)]
#[path = "remotezip_tests.rs"]
mod tests;
