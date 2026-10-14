//! `extract_host_archive` (`internal/domain/host/archive.go`): a verified
//! `tar.zst` archive extracted beneath the account home, refusing absolute,
//! traversal and escaping entries. Entries are read through [`crate::gotar`],
//! which follows Go's `archive/tar`, from a zstd stream decoded the way
//! Go's `klauspost/compress/zstd` reader surfaces it.

use crate::goerr::{path_error, quote, quote_bytes};
use crate::gotar::{self, Header};
use crate::hostread::{clean, home_dir, host_owned_path};
use serde_json::{json, Value as J};
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_ENTRIES: usize = 100_000;
const MAX_EXPANDED_BYTES: i64 = 8 * 1024 * 1024 * 1024;
const ZSTD_MAGIC: u32 = 0xFD2F_B528;

type Frame<R> = ruzstd::StreamingDecoder<io::Chain<io::Cursor<[u8; 4]>, R>, ruzstd::FrameDecoder>;

/// Where the zstd stream is: between frames (owning the source) or inside
/// one (the frame decoder owns it until the frame ends).
enum Stream<R: BufRead> {
    Between(R),
    Frame(Box<Frame<R>>),
    Taken,
}

/// The zstd frames of one file, decoded lazily, one after another, as the
/// tar reader asks for bytes. Skippable frames are skipped.
struct Zstd<R: BufRead> {
    stream: Stream<R>,
}

fn magic_mismatch() -> io::Error {
    io::Error::other("invalid input: magic number mismatch")
}

/// The Go text for a decoder failure.
fn decode_error(e: &(dyn std::error::Error + 'static)) -> io::Error {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cause {
        if let Some(io_err) = err.downcast_ref::<io::Error>() {
            if io_err.kind() == io::ErrorKind::UnexpectedEof {
                return gotar::unexpected_eof();
            }
        }
        cause = err.source();
    }
    io::Error::other(format!("zstd: {e}"))
}

/// Reads a frame's magic number. `None` is a clean end.
fn next_magic<R: Read>(source: &mut R) -> io::Result<Option<[u8; 4]>> {
    let mut magic = [0u8; 4];
    let mut n = 0;
    while n < 4 {
        match source.read(&mut magic[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    match n {
        0 => Ok(None),
        4 => Ok(Some(magic)),
        _ => Err(gotar::unexpected_eof()),
    }
}

impl<R: BufRead> Zstd<R> {
    fn new(source: R) -> Self {
        Zstd {
            stream: Stream::Between(source),
        }
    }

    /// Opens the next data frame. `false` is a clean end of the stream.
    fn open_frame(&mut self, mut source: R) -> io::Result<bool> {
        loop {
            let magic = match next_magic(&mut source) {
                Ok(Some(magic)) => magic,
                Ok(None) => {
                    self.stream = Stream::Between(source);
                    return Ok(false);
                }
                Err(e) => return Err(e),
            };
            let value = u32::from_le_bytes(magic);
            if value & 0xFFFF_FFF0 == 0x184D_2A50 {
                let mut length = [0u8; 4];
                source
                    .read_exact(&mut length)
                    .map_err(|_| gotar::unexpected_eof())?;
                let length = u64::from(u32::from_le_bytes(length));
                let skipped = io::copy(&mut (&mut source).take(length), &mut io::sink())?;
                if skipped < length {
                    return Err(gotar::unexpected_eof());
                }
                continue;
            }
            if value != ZSTD_MAGIC {
                return Err(magic_mismatch());
            }
            let decoder = ruzstd::StreamingDecoder::new(io::Cursor::new(magic).chain(source))
                .map_err(|e| decode_error(&e))?;
            self.stream = Stream::Frame(Box::new(decoder));
            return Ok(true);
        }
    }
}

impl<R: BufRead> Read for Zstd<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match std::mem::replace(&mut self.stream, Stream::Taken) {
                Stream::Taken => return Err(io::Error::other("zstd stream failed earlier")),
                Stream::Between(source) => {
                    if !self.open_frame(source)? {
                        return Ok(0);
                    }
                }
                Stream::Frame(mut frame) => match frame.read(buf) {
                    Ok(0) => {
                        let (_, source) = frame.into_inner().into_inner();
                        self.stream = Stream::Between(source);
                    }
                    Ok(n) => {
                        self.stream = Stream::Frame(frame);
                        return Ok(n);
                    }
                    Err(e) => {
                        return Err(match e.get_ref() {
                            Some(inner) => decode_error(inner),
                            None if e.kind() == io::ErrorKind::UnexpectedEof => {
                                gotar::unexpected_eof()
                            }
                            None => e,
                        });
                    }
                },
            }
        }
    }
}

fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

/// `path.Clean` on raw bytes.
fn clean_bytes(b: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(b) {
        Ok(s) => clean(s).into_bytes(),
        Err(_) => {
            // Clean works byte-wise on '/' separators; non-UTF-8 bytes in a
            // segment pass through untouched.
            let rooted = b.starts_with(b"/");
            let mut parts: Vec<&[u8]> = Vec::new();
            for seg in b.split(|&c| c == b'/') {
                match seg {
                    b"" | b"." => {}
                    b".." => {
                        if parts.last().is_some_and(|l| *l != b"..") {
                            parts.pop();
                        } else if !rooted {
                            parts.push(b"..");
                        }
                    }
                    s => parts.push(s),
                }
            }
            let joined = parts.join(&b'/');
            match (rooted, joined.is_empty()) {
                (true, _) => [b"/".as_slice(), &joined].concat(),
                (false, true) => b".".to_vec(),
                (false, false) => joined,
            }
        }
    }
}

fn unsafe_entry(raw: &[u8]) -> String {
    format!("host archive contains unsafe entry {}", quote_bytes(raw))
}

/// `safeArchiveEntryName`.
fn safe_entry_name(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.is_empty() || raw.contains(&b'\\') || raw.starts_with(b"/") {
        return Err(unsafe_entry(raw));
    }
    if raw.split(|&c| c == b'/').any(|part| part == b"..") {
        return Err(unsafe_entry(raw));
    }
    let cleaned = clean_bytes(raw);
    if cleaned == b"." || cleaned == b".." || cleaned.starts_with(b"../") {
        return Err(unsafe_entry(raw));
    }
    Ok(cleaned)
}

/// `filepath.Rel(base, target)` for two clean absolute paths: `None` when
/// the target is outside the base.
fn relative_within(base: &[u8], target: &[u8]) -> Option<Vec<u8>> {
    if target == base {
        return Some(b".".to_vec());
    }
    let prefix: Vec<u8> = if base == b"/" {
        b"/".to_vec()
    } else {
        [base, b"/"].concat()
    };
    target.strip_prefix(prefix.as_slice()).map(<[u8]>::to_vec)
}

fn join(base: &[u8], name: &[u8]) -> Vec<u8> {
    clean_bytes(&[base, b"/", name].concat())
}

/// `safeArchiveTarget`.
fn safe_target(destination: &[u8], name: &[u8]) -> Result<Vec<u8>, String> {
    let target = join(destination, name);
    let Some(relative) = relative_within(destination, &target) else {
        return Err(format!(
            "host archive entry escapes destination: {}",
            quote_bytes(name)
        ));
    };
    reject_symlink_parents(destination, &relative)?;
    Ok(target)
}

/// `rejectArchiveSymlinkParents`.
fn reject_symlink_parents(destination: &[u8], relative: &[u8]) -> Result<(), String> {
    let dir = match relative.iter().rposition(|&c| c == b'/') {
        Some(i) => clean_bytes(&relative[..i]),
        None => b".".to_vec(),
    };
    let mut current = destination.to_vec();
    for part in dir.split(|&c| c == b'/') {
        if part.is_empty() || part == b"." {
            continue;
        }
        current = join(&current, part);
        match std::fs::symlink_metadata(os(&current)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(format!(
                    "inspect host archive parent {}: {}",
                    quote_bytes(&current),
                    path_error("lstat", os(&current), &e).0
                ))
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "host archive path traverses existing symlink {}",
                    quote_bytes(&current)
                ))
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// `safeArchiveLinkTarget`.
fn safe_link_target(destination: &[u8], link: &[u8], raw: &[u8]) -> Result<Vec<u8>, String> {
    let refuse = |what: &str| {
        format!(
            "host archive {what} {} -> {}",
            quote_bytes(link),
            quote_bytes(raw)
        )
    };
    if raw.is_empty() || raw.contains(&b'\\') || raw.starts_with(b"/") {
        return Err(refuse("contains unsafe symlink"));
    }
    if raw.split(|&c| c == b'/').any(|part| part == b"..") {
        return Err(refuse("contains unsafe symlink"));
    }
    let parent = match link.iter().rposition(|&c| c == b'/') {
        Some(0) => b"/".to_vec(),
        Some(i) => link[..i].to_vec(),
        None => b".".to_vec(),
    };
    let resolved = join(&parent, raw);
    if relative_within(destination, &resolved).is_none() {
        return Err(refuse("symlink escapes destination:"));
    }
    Ok(raw.to_vec())
}

/// `rejectExistingArchiveSymlink`.
fn reject_existing_symlink(target: &[u8]) -> Result<(), String> {
    match std::fs::symlink_metadata(os(target)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "inspect host archive target {}: {}",
            quote_bytes(target),
            path_error("lstat", os(target), &e).0
        )),
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "host archive target is an existing symlink {}",
            quote_bytes(target)
        )),
        Ok(_) => Ok(()),
    }
}

fn io_text(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(_) => crate::goerr::errno_text(e),
        None => e.to_string(),
    }
}

fn mkdir_all(path: &[u8], mode: u32) -> Result<(), String> {
    crate::goerr::mkdir_all(os(path), mode).map_err(|e| e.0)
}

/// `extractZstdTar`: `(entries, expandedBytes)` or the error with the
/// counts reached.
fn extract_zstd_tar(archive: &str, destination: &[u8]) -> Result<(usize, i64), String> {
    let file =
        std::fs::File::open(archive).map_err(|e| path_error("open", Path::new(archive), &e).0)?;
    let mut reader = gotar::Reader::new(Zstd::new(BufReader::new(file)));
    let mut entries = 0usize;
    let mut expanded: i64 = 0;
    loop {
        let header: Header = match reader.next() {
            Ok(Some(h)) => h,
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        };
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(format!("archive exceeds {MAX_ENTRIES} entries"));
        }
        let name = safe_entry_name(&header.name)?;
        let target = safe_target(destination, &name)?;
        let mode = (header.mode as u32) & 0o777;
        match header.typeflag {
            gotar::TYPE_DIR => {
                reject_existing_symlink(&target)?;
                mkdir_all(&target, mode)?;
            }
            gotar::TYPE_SYMLINK => {
                let link = safe_link_target(destination, &target, &header.linkname)?;
                let parent = match target.iter().rposition(|&c| c == b'/') {
                    Some(0) => b"/".to_vec(),
                    Some(i) => target[..i].to_vec(),
                    None => b".".to_vec(),
                };
                mkdir_all(&parent, 0o700)?;
                match std::fs::symlink_metadata(os(&target)) {
                    Ok(existing) => {
                        if !existing.file_type().is_symlink() {
                            return Err(format!(
                                "archive symlink target {} already exists",
                                quote_bytes(&name)
                            ));
                        }
                        std::fs::remove_file(os(&target))
                            .map_err(|e| path_error("remove", os(&target), &e).0)?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(path_error("lstat", os(&target), &e).0),
                }
                std::os::unix::fs::symlink(os(&link), os(&target)).map_err(|e| {
                    format!(
                        "symlink {} {}: {}",
                        String::from_utf8_lossy(&link),
                        String::from_utf8_lossy(&target),
                        io_text(&e)
                    )
                })?;
            }
            gotar::TYPE_REG | gotar::TYPE_REG_A => {
                reject_existing_symlink(&target)?;
                if header.size < 0 || expanded > MAX_EXPANDED_BYTES - header.size {
                    return Err(format!(
                        "archive expanded content exceeds {MAX_EXPANDED_BYTES} bytes"
                    ));
                }
                let parent = match target.iter().rposition(|&c| c == b'/') {
                    Some(0) => b"/".to_vec(),
                    Some(i) => target[..i].to_vec(),
                    None => b".".to_vec(),
                };
                mkdir_all(&parent, 0o700)?;
                let written = write_entry(&target, mode, &mut reader, header.size)?;
                if written != header.size {
                    return Err(format!(
                        "archive entry {} has truncated content",
                        quote_bytes(&name)
                    ));
                }
                expanded += written;
            }
            other => {
                return Err(format!(
                    "archive entry {} has unsupported type {other}",
                    quote_bytes(&name)
                ))
            }
        }
    }
    if entries == 0 {
        return Err("host archive is empty".into());
    }
    Ok((entries, expanded))
}

/// `os.OpenFile(target, O_CREATE|O_TRUNC|O_WRONLY, mode)` and
/// `io.Copy(output, io.LimitReader(reader, size+1))`.
fn write_entry<R: Read>(
    target: &[u8],
    mode: u32,
    reader: &mut R,
    size: i64,
) -> Result<i64, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(os(target))
        .map_err(|e| path_error("open", os(target), &e).0)?;
    let mut limited = reader.take((size as u64).saturating_add(1));
    let mut buf = vec![0u8; 32 * 1024];
    let mut written: i64 = 0;
    loop {
        let n = match limited.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        output
            .write_all(&buf[..n])
            .map_err(|e| path_error("write", os(target), &e).0)?;
        written += n as i64;
    }
    drop(output);
    Ok(written)
}

/// `Service.ExtractHostArchive`.
pub fn extract_host_archive(
    owner: &crate::hostfiles::Owner,
    archive_path: &str,
    destination: &str,
    format: &str,
) -> Result<J, String> {
    crate::hostfiles::require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "extract_host_archive",
    )?;
    let format = format.trim().to_lowercase();
    let format = if format.is_empty() {
        "tar.zst".to_string()
    } else {
        format
    };
    if format != "tar.zst" {
        return Err(format!(
            "unsupported host archive format {}",
            quote(&format)
        ));
    }
    let home = home_dir().map_err(|e| format!("resolve home directory: {e}"))?;
    let archive = host_owned_path(&home, archive_path)?;
    match std::fs::metadata(&archive) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err("inspect host archive: archive is not a regular file".into()),
        Err(e) => {
            return Err(format!(
                "inspect host archive: {}",
                path_error("stat", Path::new(&archive), &e).0
            ))
        }
    }
    let destination = host_owned_path(&home, destination)?;
    crate::goerr::mkdir_all(Path::new(&destination), 0o700)
        .map_err(|e| format!("create host archive destination: {}", e.0))?;
    let (entries, expanded) = extract_zstd_tar(&archive, destination.as_bytes())
        .map_err(|e| format!("extract host archive: {e}"))?;
    Ok(json!({
        "archivePath": archive,
        "destination": destination,
        "format": format,
        "entries": entries,
        "expandedBytes": expanded,
        "changed": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_follow_go() {
        assert_eq!(safe_entry_name(b"a/./b/").unwrap(), b"a/b");
        assert!(safe_entry_name(b"a/../b").is_err());
        assert!(safe_entry_name(b"/a").is_err());
        assert!(safe_entry_name(b"a\\b").is_err());
        assert!(safe_entry_name(b"./").is_err());
        assert_eq!(
            safe_entry_name(b"../x").unwrap_err(),
            "host archive contains unsafe entry \"../x\""
        );
    }
}
