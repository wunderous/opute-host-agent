//! A reader for tar archives that follows Go's `archive/tar` (Go 1.25.4)
//! decision for decision: which headers it accepts, how PAX, GNU long-name
//! and sparse metadata apply, and which error ends the stream. The host
//! domain extracts recipe archives through Go's reader, so what an archive
//! means, and the error text a broken one produces, are part of the
//! contract.
//!
//! Errors are `io::Error`s whose text is Go's `err.Error()`. A clean end of
//! the archive is `Ok(None)` from [`Reader::next`].

use std::collections::BTreeMap;
use std::io::{self, Read};

const BLOCK: usize = 512;
const MAX_SPECIAL_FILE_SIZE: usize = 1 << 20;

pub const TYPE_REG: u8 = b'0';
pub const TYPE_REG_A: u8 = 0;
pub const TYPE_LINK: u8 = b'1';
pub const TYPE_SYMLINK: u8 = b'2';
pub const TYPE_CHAR: u8 = b'3';
pub const TYPE_BLOCK: u8 = b'4';
pub const TYPE_DIR: u8 = b'5';
pub const TYPE_FIFO: u8 = b'6';
const TYPE_X_HEADER: u8 = b'x';
const TYPE_X_GLOBAL_HEADER: u8 = b'g';
const TYPE_GNU_SPARSE: u8 = b'S';
const TYPE_GNU_LONG_NAME: u8 = b'L';
const TYPE_GNU_LONG_LINK: u8 = b'K';

fn go_error(text: &str) -> io::Error {
    io::Error::other(text.to_string())
}

/// `tar.ErrHeader`.
pub fn err_header() -> io::Error {
    go_error("archive/tar: invalid tar header")
}

/// `io.ErrUnexpectedEOF`.
pub fn unexpected_eof() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF")
}

fn err_field_too_long() -> io::Error {
    go_error("archive/tar: header field too long")
}

/// The header fields extraction reads.
#[derive(Debug, Clone, Default)]
pub struct Header {
    pub typeflag: u8,
    pub name: Vec<u8>,
    pub linkname: Vec<u8>,
    pub size: i64,
    pub mode: i64,
    pax: BTreeMap<Vec<u8>, Vec<u8>>,
}

// Go's Format bits.
const FORMAT_UNKNOWN: u8 = 0;
const FORMAT_V7: u8 = 1;
const FORMAT_USTAR: u8 = 2;
const FORMAT_PAX: u8 = 4;
const FORMAT_GNU: u8 = 8;
const FORMAT_STAR: u8 = 16;

#[derive(Clone, Copy)]
struct SparseEntry {
    offset: i64,
    length: i64,
}

impl SparseEntry {
    fn end(&self) -> i64 {
        self.offset.wrapping_add(self.length)
    }
}

enum Current {
    Regular {
        nb: i64,
    },
    Sparse {
        nb: i64,
        holes: Vec<SparseEntry>,
        pos: i64,
    },
}

impl Current {
    fn physical_remaining(&self) -> i64 {
        match self {
            Current::Regular { nb } | Current::Sparse { nb, .. } => *nb,
        }
    }
}

/// `tr.err`: the end of the archive, or the first error, sticks.
enum State {
    Open,
    Ended,
    Failed(io::ErrorKind, String),
}

pub struct Reader<R: Read> {
    r: R,
    pad: i64,
    curr: Current,
    blk: [u8; BLOCK],
    state: State,
    /// An error to report after the bytes read before it were returned, as
    /// Go's `(n, err)` hands both to `io.Copy`.
    pending: Option<io::Error>,
}

/// Outcome of `io.ReadFull`-style reads.
enum Fill {
    Full,
    /// No byte was read before EOF.
    Eof,
}

/// `tryReadFull`: `Ok(n)` with `n < buf.len()` means EOF after `n` bytes.
fn try_read_full<R: Read + ?Sized>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => return Ok(n),
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// `io.ReadFull`: EOF before any byte is `Fill::Eof`, a partial read is
/// `io.ErrUnexpectedEOF`.
fn read_full<R: Read + ?Sized>(r: &mut R, buf: &mut [u8]) -> io::Result<Fill> {
    match try_read_full(r, buf)? {
        n if n == buf.len() => Ok(Fill::Full),
        0 => Ok(Fill::Eof),
        _ => Err(unexpected_eof()),
    }
}

/// `mustReadFull`: any short read is `io.ErrUnexpectedEOF`.
fn must_read_full<R: Read + ?Sized>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    if try_read_full(r, buf)? == buf.len() {
        Ok(())
    } else {
        Err(unexpected_eof())
    }
}

/// `bytes.IndexByte(b, 0)` cut.
fn parse_string(b: &[u8]) -> Vec<u8> {
    match b.iter().position(|&c| c == 0) {
        Some(i) => b[..i].to_vec(),
        None => b.to_vec(),
    }
}

/// Go's `parser`: numeric fields record their first error.
#[derive(Default)]
struct Parser {
    failed: bool,
}

impl Parser {
    fn parse_numeric(&mut self, b: &[u8]) -> i64 {
        if !b.is_empty() && b[0] & 0x80 != 0 {
            let inv: u8 = if b[0] & 0x40 != 0 { 0xff } else { 0 };
            let mut x: u64 = 0;
            for (i, &c) in b.iter().enumerate() {
                let mut c = c ^ inv;
                if i == 0 {
                    c &= 0x7f;
                }
                if x >> 56 > 0 {
                    self.failed = true;
                    return 0;
                }
                x = x << 8 | u64::from(c);
            }
            if x >> 63 > 0 {
                self.failed = true;
                return 0;
            }
            return if inv == 0xff { !(x as i64) } else { x as i64 };
        }
        self.parse_octal(b)
    }

    fn parse_octal(&mut self, b: &[u8]) -> i64 {
        let trimmed = trim_bytes(b, b" \0");
        if trimmed.is_empty() {
            return 0;
        }
        let text = parse_string(trimmed);
        match std::str::from_utf8(&text)
            .ok()
            .and_then(|s| parse_uint(s, 8))
        {
            Some(x) => x as i64,
            None => {
                self.failed = true;
                // strconv.ParseUint returns the max value on range errors.
                if text.iter().all(|c| (b'0'..=b'7').contains(c)) && !text.is_empty() {
                    u64::MAX as i64
                } else {
                    0
                }
            }
        }
    }
}

fn trim_bytes<'a>(b: &'a [u8], cut: &[u8]) -> &'a [u8] {
    let start = b.iter().position(|c| !cut.contains(c)).unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !cut.contains(c))
        .map_or(start, |i| i + 1);
    &b[start..end.max(start)]
}

/// `strconv.ParseUint(s, base, 64)` for an explicit base: digits only, no
/// sign, no underscores.
fn parse_uint(s: &str, base: u32) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut x: u64 = 0;
    for c in s.chars() {
        let d = c.to_digit(base)?;
        x = x.checked_mul(u64::from(base))?.checked_add(u64::from(d))?;
    }
    Some(x)
}

/// `strconv.ParseInt(s, 10, 64)`.
fn parse_int(s: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(s).ok()?;
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let magnitude = parse_uint(digits, 10)?;
    if negative {
        if magnitude > 1 << 63 {
            None
        } else {
            Some((magnitude as i64).wrapping_neg())
        }
    } else if magnitude > i64::MAX as u64 {
        None
    } else {
        Some(magnitude as i64)
    }
}

/// `parsePAXTime`: only whether it parses matters here.
fn pax_time_valid(s: &[u8]) -> bool {
    let (secs, nanos) = match s.iter().position(|&c| c == b'.') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &b""[..]),
    };
    parse_int(secs).is_some() && nanos.iter().all(u8::is_ascii_digit)
}

fn compute_checksum(b: &[u8; BLOCK]) -> (i64, i64) {
    let (mut unsigned, mut signed) = (0i64, 0i64);
    for (i, &c) in b.iter().enumerate() {
        let c = if (148..156).contains(&i) { b' ' } else { c };
        unsigned += i64::from(c);
        signed += i64::from(c as i8);
    }
    (unsigned, signed)
}

fn get_format(b: &[u8; BLOCK]) -> u8 {
    let mut p = Parser::default();
    let value = p.parse_octal(&b[148..156]);
    let (unsigned, signed) = compute_checksum(b);
    if p.failed || (value != unsigned && value != signed) {
        return FORMAT_UNKNOWN;
    }
    let magic = &b[257..263];
    let version = &b[263..265];
    let trailer = &b[508..512];
    if magic == b"ustar\0" && trailer == b"tar\0" {
        FORMAT_STAR
    } else if magic == b"ustar\0" {
        FORMAT_USTAR | FORMAT_PAX
    } else if magic == b"ustar " && version == b" \0" {
        FORMAT_GNU
    } else {
        FORMAT_V7
    }
}

fn is_header_only_type(flag: u8) -> bool {
    matches!(
        flag,
        TYPE_LINK | TYPE_SYMLINK | TYPE_CHAR | TYPE_BLOCK | TYPE_DIR | TYPE_FIFO
    )
}

fn block_padding(offset: i64) -> i64 {
    offset.wrapping_neg() & (BLOCK as i64 - 1)
}

/// `parsePAXRecord`.
fn parse_pax_record(s: &[u8]) -> Option<(Vec<u8>, Vec<u8>, &[u8])> {
    let space = s.iter().position(|&c| c == b' ')?;
    let (n_str, rest) = (&s[..space], &s[space + 1..]);
    let n = parse_int(n_str)?;
    if n < 5 || n > s.len() as i64 {
        return None;
    }
    let n = n - (n_str.len() as i64 + 1);
    if n <= 0 {
        return None;
    }
    let n = n as usize;
    let (record, newline, remainder) = (&rest[..n - 1], &rest[n - 1..n], &rest[n..]);
    if newline != b"\n" {
        return None;
    }
    let eq = record.iter().position(|&c| c == b'=')?;
    let (key, value) = (&record[..eq], &record[eq + 1..]);
    // validPAXRecord.
    if key.is_empty() || key.contains(&b'=') {
        return None;
    }
    let path_like = matches!(key, b"path" | b"linkpath" | b"uname" | b"gname");
    if (path_like && value.contains(&0)) || (!path_like && key.contains(&0)) {
        return None;
    }
    Some((key.to_vec(), value.to_vec(), remainder))
}

impl<R: Read> Reader<R> {
    pub fn new(r: R) -> Self {
        Reader {
            r,
            pad: 0,
            curr: Current::Regular { nb: 0 },
            blk: [0; BLOCK],
            state: State::Open,
            pending: None,
        }
    }

    fn sticky(&self) -> Option<io::Error> {
        match &self.state {
            State::Failed(kind, text) => Some(io::Error::new(*kind, text.clone())),
            _ => None,
        }
    }

    fn remember(&mut self, e: &io::Error) {
        self.state = State::Failed(e.kind(), e.to_string());
    }

    /// `Reader.Next`: the next entry, or `None` at the end of the archive.
    pub fn next(&mut self) -> io::Result<Option<Header>> {
        if matches!(self.state, State::Ended) {
            return Ok(None);
        }
        if let Some(e) = self.sticky() {
            return Err(e);
        }
        self.pending = None;
        match self.next_inner() {
            Ok(Some(h)) => Ok(Some(h)),
            Ok(None) => {
                self.state = State::Ended;
                Ok(None)
            }
            Err(e) => {
                self.remember(&e);
                Err(e)
            }
        }
    }

    fn next_inner(&mut self) -> io::Result<Option<Header>> {
        let mut pax_hdrs: Option<BTreeMap<Vec<u8>, Vec<u8>>> = None;
        let mut gnu_long_name = Vec::new();
        let mut gnu_long_link = Vec::new();
        loop {
            let remaining = self.curr.physical_remaining();
            self.discard(remaining)?;
            let pad = self.pad as usize;
            let mut padding = vec![0u8; pad];
            if try_read_full(&mut self.r, &mut padding)? < pad {
                return Ok(None);
            }
            self.pad = 0;

            let Some((mut hdr, raw)) = self.read_header()? else {
                return Ok(None);
            };
            self.handle_regular_file(&hdr)?;
            match hdr.typeflag {
                TYPE_X_HEADER | TYPE_X_GLOBAL_HEADER => {
                    let records = self.parse_pax()?;
                    if hdr.typeflag == TYPE_X_GLOBAL_HEADER {
                        // mergePAX's error is ignored for a global header.
                        let _ = merge_pax(&mut hdr, &records);
                        return Ok(Some(Header {
                            typeflag: hdr.typeflag,
                            name: hdr.name,
                            pax: records,
                            ..Header::default()
                        }));
                    }
                    pax_hdrs = Some(records);
                    continue;
                }
                TYPE_GNU_LONG_NAME | TYPE_GNU_LONG_LINK => {
                    let realname = self.read_special_file()?;
                    if hdr.typeflag == TYPE_GNU_LONG_NAME {
                        gnu_long_name = parse_string(&realname);
                    } else {
                        gnu_long_link = parse_string(&realname);
                    }
                    continue;
                }
                _ => {
                    merge_pax(&mut hdr, pax_hdrs.as_ref().unwrap_or(&BTreeMap::new()))?;
                    if !gnu_long_name.is_empty() {
                        hdr.name = gnu_long_name;
                    }
                    if !gnu_long_link.is_empty() {
                        hdr.linkname = gnu_long_link;
                    }
                    if hdr.typeflag == TYPE_REG_A {
                        hdr.typeflag = if hdr.name.ends_with(b"/") {
                            TYPE_DIR
                        } else {
                            TYPE_REG
                        };
                    }
                    self.handle_regular_file(&hdr)?;
                    self.handle_sparse_file(&mut hdr, &raw)?;
                    return Ok(Some(hdr));
                }
            }
        }
    }

    /// `discard`: skip the rest of the current entry.
    fn discard(&mut self, n: i64) -> io::Result<()> {
        if n <= 0 {
            return Ok(());
        }
        let copied = io::copy(&mut (&mut self.r).take(n as u64), &mut io::sink())?;
        if (copied as i64) < n {
            return Err(unexpected_eof());
        }
        Ok(())
    }

    fn handle_regular_file(&mut self, hdr: &Header) -> io::Result<()> {
        let nb = if is_header_only_type(hdr.typeflag) {
            0
        } else {
            hdr.size
        };
        if nb < 0 {
            return Err(err_header());
        }
        self.pad = block_padding(nb);
        self.curr = Current::Regular { nb };
        Ok(())
    }

    /// `readHeader`. `None` is the end of the archive.
    fn read_header(&mut self) -> io::Result<Option<(Header, [u8; BLOCK])>> {
        if let Fill::Eof = read_full(&mut self.r, &mut self.blk)? {
            return Ok(None);
        }
        if self.blk.iter().all(|&c| c == 0) {
            if let Fill::Eof = read_full(&mut self.r, &mut self.blk)? {
                return Ok(None);
            }
            if self.blk.iter().all(|&c| c == 0) {
                return Ok(None);
            }
            return Err(err_header());
        }
        let format = get_format(&self.blk);
        if format == FORMAT_UNKNOWN {
            return Err(err_header());
        }
        let b = self.blk;
        let mut p = Parser::default();
        let mut hdr = Header {
            typeflag: b[156],
            name: parse_string(&b[0..100]),
            linkname: parse_string(&b[157..257]),
            size: p.parse_numeric(&b[124..136]),
            mode: p.parse_numeric(&b[100..108]),
            pax: BTreeMap::new(),
        };
        p.parse_numeric(&b[108..116]);
        p.parse_numeric(&b[116..124]);
        p.parse_numeric(&b[136..148]);
        if format > FORMAT_V7 {
            p.parse_numeric(&b[329..337]);
            p.parse_numeric(&b[337..345]);
            let mut prefix = Vec::new();
            if format & (FORMAT_USTAR | FORMAT_PAX) != 0 {
                prefix = parse_string(&b[345..500]);
            } else if format & FORMAT_STAR != 0 {
                prefix = parse_string(&b[345..476]);
                p.parse_numeric(&b[476..488]);
                p.parse_numeric(&b[488..500]);
            } else if format & FORMAT_GNU != 0 {
                let mut p2 = Parser::default();
                if b[345] != 0 {
                    p2.parse_numeric(&b[345..357]);
                }
                if b[357] != 0 {
                    p2.parse_numeric(&b[357..369]);
                }
                if p2.failed {
                    let s = parse_string(&b[345..500]);
                    if s.iter().all(|&c| c < 0x80 && c != 0) {
                        prefix = s;
                    }
                }
            }
            if !prefix.is_empty() {
                let mut name = prefix;
                name.push(b'/');
                name.extend_from_slice(&hdr.name);
                hdr.name = name;
            }
        }
        if p.failed {
            return Err(err_header());
        }
        Ok(Some((hdr, b)))
    }

    /// `readSpecialFile`: the current entry's data, at most 1 MiB.
    fn read_special_file(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let limit = (MAX_SPECIAL_FILE_SIZE + 1) as u64;
        let result = self.take_current(limit, &mut buf);
        if buf.len() > MAX_SPECIAL_FILE_SIZE {
            return Err(err_field_too_long());
        }
        result.map(|_| buf)
    }

    /// `io.ReadAll(io.LimitReader(tr, limit))`.
    fn take_current(&mut self, limit: u64, buf: &mut Vec<u8>) -> io::Result<()> {
        let mut chunk = [0u8; 4096];
        while (buf.len() as u64) < limit {
            let want = chunk.len().min((limit - buf.len() as u64) as usize);
            match self.read(&mut chunk[..want]) {
                Ok(0) => return Ok(()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// `parsePAX`.
    fn parse_pax(&mut self) -> io::Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let buf = self.read_special_file()?;
        let mut s: &[u8] = &buf;
        let mut sparse_map: Vec<Vec<u8>> = Vec::new();
        let mut records = BTreeMap::new();
        while !s.is_empty() {
            let (key, value, rest) = parse_pax_record(s).ok_or_else(err_header)?;
            s = rest;
            match key.as_slice() {
                b"GNU.sparse.offset" | b"GNU.sparse.numbytes" => {
                    let even = sparse_map.len() % 2 == 0;
                    if (even && key != b"GNU.sparse.offset")
                        || (!even && key != b"GNU.sparse.numbytes")
                        || value.contains(&b',')
                    {
                        return Err(err_header());
                    }
                    sparse_map.push(value);
                }
                _ => {
                    records.insert(key, value);
                }
            }
        }
        if !sparse_map.is_empty() {
            records.insert(b"GNU.sparse.map".to_vec(), sparse_map.join(&b','));
        }
        Ok(records)
    }

    fn handle_sparse_file(&mut self, hdr: &mut Header, raw: &[u8; BLOCK]) -> io::Result<()> {
        let spd = if hdr.typeflag == TYPE_GNU_SPARSE {
            self.read_old_gnu_sparse_map(hdr, raw)?
        } else {
            self.read_gnu_sparse_pax_headers(hdr)?
        };
        if let Some(spd) = spd {
            if is_header_only_type(hdr.typeflag) || !validate_sparse_entries(&spd, hdr.size) {
                return Err(err_header());
            }
            let holes = invert_sparse_entries(spd, hdr.size);
            let nb = self.curr.physical_remaining();
            self.curr = Current::Sparse { nb, holes, pos: 0 };
        }
        Ok(())
    }

    fn read_gnu_sparse_pax_headers(
        &mut self,
        hdr: &mut Header,
    ) -> io::Result<Option<Vec<SparseEntry>>> {
        let get = |k: &[u8]| hdr.pax.get(k).cloned().unwrap_or_default();
        let (major, minor) = (get(b"GNU.sparse.major"), get(b"GNU.sparse.minor"));
        let is1x0 = match (major.as_slice(), minor.as_slice()) {
            (b"0", b"0" | b"1") => false,
            (b"1", b"0") => true,
            _ if !major.is_empty() || !minor.is_empty() => return Ok(None),
            _ if !get(b"GNU.sparse.map").is_empty() => false,
            _ => return Ok(None),
        };
        let name = get(b"GNU.sparse.name");
        if !name.is_empty() {
            hdr.name = name;
        }
        let mut size = get(b"GNU.sparse.size");
        if size.is_empty() {
            size = get(b"GNU.sparse.realsize");
        }
        if !size.is_empty() {
            hdr.size = parse_int(&size).ok_or_else(err_header)?;
        }
        if is1x0 {
            self.read_gnu_sparse_map_1x0().map(Some)
        } else {
            read_gnu_sparse_map_0x1(&hdr.pax).map(Some)
        }
    }

    fn read_gnu_sparse_map_1x0(&mut self) -> io::Result<Vec<SparseEntry>> {
        let mut newlines: i64 = 0;
        let mut buf: Vec<u8> = Vec::new();
        let mut total = 0usize;
        let mut feed = |me: &mut Self, n: i64, buf: &mut Vec<u8>, newlines: &mut i64| {
            while *newlines < n {
                total += BLOCK;
                if total > MAX_SPECIAL_FILE_SIZE {
                    return Err(go_error("archive/tar: sparse map too long"));
                }
                let mut blk = [0u8; BLOCK];
                me.must_read_current(&mut blk)?;
                buf.extend_from_slice(&blk);
                *newlines += blk.iter().filter(|&&c| c == b'\n').count() as i64;
            }
            Ok(())
        };
        let next_token = |buf: &mut Vec<u8>, newlines: &mut i64| {
            *newlines -= 1;
            let end = buf
                .iter()
                .position(|&c| c == b'\n')
                .map_or(buf.len(), |i| i + 1);
            let token: Vec<u8> = buf.drain(..end).collect();
            trim_bytes_right(&token, b"\n").to_vec()
        };
        feed(self, 1, &mut buf, &mut newlines)?;
        let entries = parse_int(&next_token(&mut buf, &mut newlines))
            .filter(|n| *n >= 0 && n.checked_mul(2).is_some())
            .ok_or_else(err_header)?;
        feed(self, 2 * entries, &mut buf, &mut newlines)?;
        let mut spd = Vec::new();
        for _ in 0..entries {
            let offset = parse_int(&next_token(&mut buf, &mut newlines));
            let length = parse_int(&next_token(&mut buf, &mut newlines));
            match (offset, length) {
                (Some(offset), Some(length)) => spd.push(SparseEntry { offset, length }),
                _ => return Err(err_header()),
            }
        }
        Ok(spd)
    }

    /// `mustReadFull(tr.curr, blk)`.
    fn must_read_current(&mut self, blk: &mut [u8]) -> io::Result<()> {
        let mut n = 0;
        while n < blk.len() {
            match self.read_current(&mut blk[n..]) {
                Ok((k, at_end)) => {
                    n += k;
                    if (k == 0 || at_end) && n < blk.len() {
                        return Err(unexpected_eof());
                    }
                }
                Err((_, e)) => return Err(e),
            }
        }
        Ok(())
    }

    fn read_old_gnu_sparse_map(
        &mut self,
        hdr: &mut Header,
        raw: &[u8; BLOCK],
    ) -> io::Result<Option<Vec<SparseEntry>>> {
        if get_format(raw) != FORMAT_GNU {
            return Err(err_header());
        }
        let mut p = Parser::default();
        hdr.size = p.parse_numeric(&raw[483..495]);
        if p.failed {
            return Err(err_header());
        }
        let mut sparse: Vec<u8> = raw[386..386 + 24 * 4 + 1].to_vec();
        let mut spd = Vec::new();
        loop {
            let max = sparse.len() / 24;
            for i in 0..max {
                let entry = &sparse[i * 24..];
                if entry[0] == 0 {
                    break;
                }
                let offset = p.parse_numeric(&entry[0..12]);
                let length = p.parse_numeric(&entry[12..24]);
                if p.failed {
                    return Err(err_header());
                }
                spd.push(SparseEntry { offset, length });
            }
            if sparse[24 * max] > 0 {
                let mut blk = [0u8; BLOCK];
                must_read_full(&mut self.r, &mut blk)?;
                sparse = blk.to_vec();
                continue;
            }
            return Ok(Some(spd));
        }
    }

    /// One read from the current entry: `(n, at end)`. A failure carries
    /// the bytes read before it.
    fn read_current(&mut self, b: &mut [u8]) -> Result<(usize, bool), (usize, io::Error)> {
        match &mut self.curr {
            Current::Regular { nb } => {
                // regFileReader.Read
                let want = (b.len() as i64).min(*nb).max(0) as usize;
                let mut n = 0;
                if want > 0 {
                    n = loop {
                        match self.r.read(&mut b[..want]) {
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err((0, e)),
                            Ok(n) => break n,
                        }
                    };
                    *nb -= n as i64;
                    if n == 0 && *nb > 0 {
                        return Err((0, unexpected_eof()));
                    }
                }
                Ok((n, *nb == 0))
            }
            Current::Sparse { nb, holes, pos } => {
                // sparseFileReader.Read over its regFileReader.
                let logical = holes.last().map_or(0, |h| h.end()) - *pos;
                let finished = b.len() as i64 >= logical;
                let want = if finished {
                    logical.max(0) as usize
                } else {
                    b.len()
                };
                let end_pos = *pos + want as i64;
                let mut done = 0usize;
                let mut failure = None;
                while end_pos > *pos && failure.is_none() {
                    let (hole_start, hole_end) = (holes[0].offset, holes[0].end());
                    let room = want - done;
                    let nf = if *pos < hole_start {
                        let bf = room.min((hole_start - *pos) as usize);
                        let mut got = 0;
                        while got < bf {
                            if *nb == 0 {
                                // The dense data ran out before the map did.
                                failure = Some(go_error(
                                    "archive/tar: sparse file references non-existent data",
                                ));
                                break;
                            }
                            let take = ((bf - got) as i64).min(*nb) as usize;
                            match self.r.read(&mut b[done + got..done + got + take]) {
                                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                                Err(e) => {
                                    failure = Some(e);
                                    break;
                                }
                                Ok(0) => {
                                    failure = Some(unexpected_eof());
                                    break;
                                }
                                Ok(k) => {
                                    got += k;
                                    *nb -= k as i64;
                                }
                            }
                        }
                        got
                    } else {
                        let bf = room.min((hole_end - *pos) as usize);
                        b[done..done + bf].fill(0);
                        bf
                    };
                    done += nf;
                    *pos += nf as i64;
                    if *pos >= hole_end && holes.len() > 1 {
                        holes.remove(0);
                    }
                }
                if let Some(e) = failure {
                    return Err((done, e));
                }
                let logical = holes.last().map_or(0, |h| h.end()) - *pos;
                if logical == 0 && *nb > 0 {
                    return Err((
                        done,
                        go_error("archive/tar: sparse file contains unreferenced data"),
                    ));
                }
                Ok((done, finished))
            }
        }
    }
}

impl<R: Read> Read for Reader<R> {
    /// `Reader.Read`: the current entry's data; `Ok(0)` at its end.
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if let Some(e) = self.pending.take() {
            self.remember(&e);
            return Err(e);
        }
        match &self.state {
            State::Ended => return Ok(0),
            State::Failed(..) => return Err(self.sticky().expect("failed state")),
            State::Open => {}
        }
        if b.is_empty() {
            return Ok(0);
        }
        match self.read_current(b) {
            Ok((n, _)) => Ok(n),
            Err((0, e)) => {
                self.remember(&e);
                Err(e)
            }
            Err((n, e)) => {
                self.pending = Some(e);
                Ok(n)
            }
        }
    }
}

fn trim_bytes_right<'a>(b: &'a [u8], cut: &[u8]) -> &'a [u8] {
    let end = b
        .iter()
        .rposition(|c| !cut.contains(c))
        .map_or(0, |i| i + 1);
    &b[..end]
}

/// `mergePAX`.
fn merge_pax(hdr: &mut Header, records: &BTreeMap<Vec<u8>, Vec<u8>>) -> io::Result<()> {
    for (k, v) in records {
        if v.is_empty() {
            continue;
        }
        let ok = match k.as_slice() {
            b"path" => {
                hdr.name = v.clone();
                true
            }
            b"linkpath" => {
                hdr.linkname = v.clone();
                true
            }
            b"uid" | b"gid" => parse_int(v).is_some(),
            b"atime" | b"mtime" | b"ctime" => pax_time_valid(v),
            b"size" => match parse_int(v) {
                Some(n) => {
                    hdr.size = n;
                    true
                }
                None => false,
            },
            _ => true,
        };
        if !ok {
            return Err(err_header());
        }
    }
    hdr.pax = records.clone();
    Ok(())
}

fn read_gnu_sparse_map_0x1(pax: &BTreeMap<Vec<u8>, Vec<u8>>) -> io::Result<Vec<SparseEntry>> {
    let get = |k: &[u8]| pax.get(k).cloned().unwrap_or_default();
    let entries = parse_int(&get(b"GNU.sparse.numblocks"))
        .filter(|n| *n >= 0 && n.checked_mul(2).is_some())
        .ok_or_else(err_header)?;
    let map = get(b"GNU.sparse.map");
    let mut parts: Vec<&[u8]> = map.split(|&c| c == b',').collect();
    if parts.len() == 1 && parts[0].is_empty() {
        parts.clear();
    }
    if parts.len() as i64 != 2 * entries {
        return Err(err_header());
    }
    let mut spd = Vec::new();
    for pair in parts.chunks(2) {
        match (parse_int(pair[0]), parse_int(pair[1])) {
            (Some(offset), Some(length)) => spd.push(SparseEntry { offset, length }),
            _ => return Err(err_header()),
        }
    }
    Ok(spd)
}

fn validate_sparse_entries(sp: &[SparseEntry], size: i64) -> bool {
    if size < 0 {
        return false;
    }
    let mut pre = SparseEntry {
        offset: 0,
        length: 0,
    };
    for cur in sp {
        if cur.offset < 0 || cur.length < 0 {
            return false;
        }
        if cur.offset > i64::MAX - cur.length {
            return false;
        }
        if cur.end() > size || pre.end() > cur.offset {
            return false;
        }
        pre = *cur;
    }
    true
}

fn invert_sparse_entries(src: Vec<SparseEntry>, size: i64) -> Vec<SparseEntry> {
    let mut dst = Vec::new();
    let mut pre = SparseEntry {
        offset: 0,
        length: 0,
    };
    for cur in src {
        if cur.length == 0 {
            continue;
        }
        pre.length = cur.offset - pre.offset;
        if pre.length > 0 {
            dst.push(pre);
        }
        pre.offset = cur.end();
    }
    pre.length = size - pre.offset;
    dst.push(pre);
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_fields_follow_go() {
        let mut p = Parser::default();
        assert_eq!(p.parse_numeric(b"0000644\0"), 0o644);
        assert_eq!(p.parse_numeric(b"  17 \0"), 0o17);
        assert_eq!(p.parse_numeric(b"\0\0\0"), 0);
        assert!(!p.failed);
        assert_eq!(p.parse_numeric(&[0x80, 0, 0, 0, 0, 0, 0, 0x2a]), 42);
        assert_eq!(p.parse_numeric(&[0xff; 8]), -1);
        p.parse_numeric(b"0009\0");
        assert!(p.failed);
    }

    #[test]
    fn pax_records_follow_go() {
        let (k, v, rest) = parse_pax_record(b"19 path=/etc/hosts\nXX").unwrap();
        assert_eq!(
            (k.as_slice(), v.as_slice(), rest),
            (&b"path"[..], &b"/etc/hosts"[..], &b"XX"[..])
        );
        assert!(parse_pax_record(b"18 path=/etc/hosts\n").is_none());
        assert!(parse_pax_record(b"4 a=\n").is_none());
        assert!(parse_pax_record(b"8 =abc\n\n").is_none());
    }

    #[test]
    fn sparse_maps_invert_like_go() {
        let holes = invert_sparse_entries(
            vec![
                SparseEntry {
                    offset: 0,
                    length: 5,
                },
                SparseEntry {
                    offset: 10,
                    length: 3,
                },
            ],
            16,
        );
        let ends: Vec<(i64, i64)> = holes.iter().map(|h| (h.offset, h.length)).collect();
        assert_eq!(ends, vec![(5, 5), (13, 3)]);
    }
}
