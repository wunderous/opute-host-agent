//! Error text compatible with the Go baseline.
//!
//! Error messages are part of the observable contract: the CLI prints them
//! verbatim and the parity harness compares them byte for byte. This module
//! reproduces the few Go formatting rules the Host Agent's messages depend on:
//! `os.PathError` ("op path: reason"), syscall reason text, `%q` quoting, and
//! `os.MkdirAll`'s choice of which path to report.

mod printable;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// A plain error carrying an already formatted Go-compatible message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// `fmt.Errorf(...)` equivalent.
#[macro_export]
macro_rules! go_err {
    ($($arg:tt)*) => { $crate::goerr::Error(format!($($arg)*)) };
}

/// Go's syscall error text: the C strerror string with a lowercase first
/// letter (Go's `syscall.Errno.Error` table), e.g. "no such file or directory".
pub fn errno_text(err: &io::Error) -> String {
    match err.raw_os_error() {
        Some(code) => {
            let text = io::Error::from_raw_os_error(code).to_string();
            let text = text
                .rsplit_once(" (os error ")
                .map(|(t, _)| t.to_string())
                .unwrap_or(text);
            lowercase_first(&text)
        }
        None => err.to_string(),
    }
}

fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `os.PathError.Error()`: "op path: reason".
pub fn path_error(op: &str, path: &Path, err: &io::Error) -> Error {
    Error(format!("{op} {}: {}", path.display(), errno_text(err)))
}

/// `os.CreateTemp(dir, prefix + "*" + suffix)`: a new 0600 file whose name
/// carries a random decimal `uint32`, returned with that name.
///
/// As in Go, any failure other than a name collision is the `OpenFile`
/// error for the name tried ("open <dir>/<prefix><n><suffix>: <reason>");
/// only 10000 consecutive collisions report the pattern ("createtemp ...*").
pub fn create_temp(
    dir: &str,
    prefix: &str,
    suffix: &str,
) -> std::result::Result<(std::fs::File, String), String> {
    use std::os::unix::fs::OpenOptionsExt;
    // `joinPath`: no separator is added after a trailing one.
    let joined = if dir.ends_with('/') {
        format!("{dir}{prefix}")
    } else {
        format!("{dir}/{prefix}")
    };
    for _ in 0..10_000 {
        let mut seed = [0u8; 4];
        getrandom::getrandom(&mut seed).map_err(|e| e.to_string())?;
        let name = format!("{joined}{}{suffix}", u32::from_ne_bytes(seed));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&name)
        {
            Ok(file) => return Ok((file, name)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("open {name}: {}", errno_text(&e))),
        }
    }
    Err(format!("createtemp {joined}*{suffix}: file already exists"))
}

/// `os.MkdirAll(path, perm)` with Go's error reporting.
///
/// Go reports the component that failed, not always the requested path, and
/// reports ENOTDIR when the path exists but is not a directory.
pub fn mkdir_all(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(path_error(
                "mkdir",
                path,
                &io::Error::from_raw_os_error(libc_enotdir()),
            ))
        }
        Err(_) => {}
    }
    // Go strips trailing separators, then creates the parent first.
    let trimmed: PathBuf = {
        let s = path.as_os_str().to_string_lossy();
        let t = s.trim_end_matches('/');
        PathBuf::from(if t.is_empty() { "/" } else { t })
    };
    if let Some(parent) = trimmed.parent() {
        if !parent.as_os_str().is_empty() && parent != trimmed {
            mkdir_all(parent, mode)?;
        }
    }
    match std::fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Handle the race / "already exists as a directory" case like Go.
            if std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
                Ok(())
            } else {
                Err(path_error("mkdir", path, &err))
            }
        }
    }
}

const fn libc_enotdir() -> i32 {
    20 // ENOTDIR on Linux
}

/// Go `strconv.Quote` / `%q` for the strings the Host Agent formats.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if !printable::is_print(c) => {
                if (c as u32) <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                } else {
                    out.push_str(&format!("\\U{:08x}", c as u32));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Go `%q` for a byte string: valid UTF-8 runs as [`quote`], each invalid
/// byte as `\xNN`.
pub fn quote_bytes(b: &[u8]) -> String {
    let mut out = String::from("\"");
    for chunk in b.utf8_chunks() {
        let valid = quote(chunk.valid());
        out.push_str(&valid[1..valid.len() - 1]);
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out.push('"');
    out
}

/// Go `strconv.ParseFloat(s, 64)` for decimal input (signs, exponents,
/// `inf`, `infinity`, `nan`). Hexadecimal floats and digit separators are
/// rejected here; no capacity argument uses them.
pub fn parse_float(s: &str) -> Option<f64> {
    if s.is_empty() || s.contains(['_', 'x', 'X', 'p', 'P']) {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Go `strconv.Atoi` result: the value Go would produce plus whether it
/// reported an error. On overflow Go returns the clamped value with an error.
pub fn atoi(s: &str) -> (i64, bool) {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (0, true);
    }
    let mut value: i128 = 0;
    for b in digits.bytes() {
        value = value * 10 + i128::from(b - b'0');
        if value > i128::from(i64::MAX) + 1 {
            break;
        }
    }
    let value = if neg { -value } else { value };
    if value > i128::from(i64::MAX) {
        (i64::MAX, true)
    } else if value < i128::from(i64::MIN) {
        (i64::MIN, true)
    } else {
        (value as i64, false)
    }
}

/// `textutil.ShellQuote`: single-quoted, with embedded quotes escaped.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_go() {
        assert_eq!(quote("bogus"), "\"bogus\"");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote("x\ty\n"), "\"x\\ty\\n\"");
        assert_eq!(quote("\u{1}"), "\"\\x01\"");
        assert_eq!(quote("héllo"), "\"héllo\"");
    }

    #[test]
    fn create_temp_reports_the_name_it_tried_like_go() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("goerr-create-temp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.display().to_string();
        let (_, name) = create_temp(&format!("{base}/"), ".x-", ".tmp").unwrap();
        let random = name
            .strip_prefix(&format!("{base}/.x-"))
            .and_then(|n| n.strip_suffix(".tmp"))
            .unwrap();
        assert!(random.parse::<u32>().is_ok(), "{name}");
        assert_eq!(
            std::fs::metadata(&name).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Go: os.CreateTemp returns OpenFile's PathError for the attempted
        // name, never the "createtemp" pattern, unless names are exhausted.
        let missing = format!("{base}/missing");
        let err = create_temp(&missing, ".y-", "").unwrap_err();
        let tried = err
            .strip_prefix(&format!("open {missing}/.y-"))
            .and_then(|e| e.strip_suffix(": no such file or directory"))
            .unwrap_or_else(|| panic!("{err}"));
        assert!(tried.parse::<u32>().is_ok(), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atoi_matches_go() {
        assert_eq!(atoi("3014"), (3014, false));
        assert_eq!(atoi("+7"), (7, false));
        assert_eq!(atoi("-7"), (-7, false));
        assert_eq!(atoi("007"), (7, false));
        assert_eq!(atoi(""), (0, true));
        assert_eq!(atoi("12a"), (0, true));
        assert_eq!(atoi(" 1"), (0, true));
        assert_eq!(atoi("99999999999999999999"), (i64::MAX, true));
        assert_eq!(atoi("-99999999999999999999"), (i64::MIN, true));
    }

    #[test]
    fn errno_text_is_go_style() {
        let err = io::Error::from_raw_os_error(2);
        assert_eq!(errno_text(&err), "no such file or directory");
        let err = io::Error::from_raw_os_error(98);
        assert_eq!(errno_text(&err), "address already in use");
    }

    #[test]
    fn mkdir_all_reports_failing_component() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        let target = file.join("a").join("b");
        // Go recurses to the first existing ancestor, which is the file.
        let err = mkdir_all(&target, 0o700).unwrap_err();
        assert_eq!(err.0, format!("mkdir {}: not a directory", file.display()));
        let err = mkdir_all(&file, 0o700).unwrap_err();
        assert_eq!(err.0, format!("mkdir {}: not a directory", file.display()));
    }
}
