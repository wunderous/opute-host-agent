//! HTTP/1.x server with Go `net/http` semantics (go1.25, the toolchain that
//! built the pinned reference binary).
//!
//! The agent's wire contract includes how the *server* treats malformed or
//! unusual requests: Go answers `HTTP/9.9` with 505 and a text body, a missing
//! `Host` with `400 Bad Request: missing required Host header`, `OPTIONS *`
//! with 200, and so on. Off-the-shelf servers answer those cases themselves
//! with different statuses and bodies, so this module ports the relevant part
//! of `net/http` instead:
//!
//! ```text
//!   read head (limit 1 MiB + 4 KiB, else 431)
//!     -> request line, version, URI, MIME headers  (Go's error texts)
//!     -> Host / header-name / header-value checks
//!     -> body framing: Content-Length | chunked | other TE -> 501
//!     -> Expect: 100-continue, other Expect -> 417
//!     -> handler (complete response) -> Content-Length, Date, keep-alive
//!     -> discard up to 256 KiB of unread body, else close
//! ```
//!
//! Handlers return complete responses (this agent never streams), so Go's
//! 2 KiB "buffer before chunking" detail is not reproduced: every response
//! carries a Content-Length, which is equally valid HTTP/1.1.

use std::future::Future;
use std::net::SocketAddr;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

const MAX_HEADER_BYTES: usize = (1 << 20) + 4096;
const MAX_POST_HANDLER_READ_BYTES: u64 = 256 << 10;

/// Ordered, canonical-key header map with repeated values (`http.Header`).
#[derive(Clone, Debug, Default)]
pub struct Headers(Vec<(String, Vec<String>)>);

impl Headers {
    pub fn get(&self, key: &str) -> &str {
        let key = canonical_key(key);
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.first())
            .map_or("", |s| s.as_str())
    }

    pub fn values(&self, key: &str) -> &[String] {
        let key = canonical_key(key);
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(&[][..], |(_, v)| v.as_slice())
    }

    pub fn has(&self, key: &str) -> bool {
        let key = canonical_key(key);
        self.0.iter().any(|(k, _)| *k == key)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let key = canonical_key(key);
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = vec![value],
            None => self.0.push((key, vec![value])),
        }
    }

    pub fn del(&mut self, key: &str) {
        let key = canonical_key(key);
        self.0.retain(|(k, _)| *k != key);
    }

    fn add_raw(&mut self, key: String, value: String) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(value),
            None => self.0.push((key, vec![value])),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }
}

/// `textproto.CanonicalMIMEHeaderKey` for keys built by this crate.
pub fn canonical_key(key: &str) -> String {
    if !key.bytes().all(valid_header_field_byte) {
        return key.to_string();
    }
    let mut upper = true;
    key.bytes()
        .map(|c| {
            let out = if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            upper = c == b'-';
            out as char
        })
        .collect()
}

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub request_uri: String,
    /// `URL.EscapedPath()`.
    pub escaped_path: String,
    pub raw_query: String,
    pub proto_major: u8,
    pub proto_minor: u8,
    pub headers: Headers,
    pub host: String,
    pub local_addr: SocketAddr,
    pub remote_addr: SocketAddr,
    framing: Framing,
    expects_continue: bool,
}

impl Request {
    pub fn proto_at_least(&self, major: u8, minor: u8) -> bool {
        self.proto_major > major || (self.proto_major == major && self.proto_minor >= minor)
    }

    /// `URL.Query().Get(key)`.
    pub fn query_get(&self, key: &str) -> String {
        form_get(&self.raw_query, key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Framing {
    None,
    Length(u64),
    Chunked,
}

pub struct Response {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Response {
            status,
            headers: Headers::default(),
            body: Vec::new(),
        }
    }

    /// `http.Error`.
    pub fn error(message: &str, status: u16) -> Self {
        let mut r = Response::new(status);
        r.headers.set("Content-Type", "text/plain; charset=utf-8");
        r.headers.set("X-Content-Type-Options", "nosniff");
        r.body = format!("{message}\n").into_bytes();
        r
    }

    /// `json.NewEncoder(w).Encode(v)` after setting Content-Type.
    pub fn json(status: u16, body: Vec<u8>) -> Self {
        let mut r = Response::new(status);
        r.headers.set("Content-Type", "application/json");
        r.body = body;
        r.body.push(b'\n');
        r
    }
}

/// Request body reader with Go's framing and 100-continue behaviour.
pub struct Body {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    framing: Framing,
    remaining: u64,
    chunk_left: u64,
    done: bool,
    continue_pending: bool,
    failed: bool,
    read_any: bool,
}

#[derive(Debug)]
pub enum BodyError {
    TooLarge,
    /// The read error's Go text (`unexpected EOF`, chunked-encoding errors).
    Io(String),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyError::TooLarge => f.write_str("http: request body too large"),
            BodyError::Io(m) => f.write_str(m),
        }
    }
}

fn eof() -> BodyError {
    BodyError::Io("unexpected EOF".into())
}

impl Body {
    /// `io.ReadAll`, optionally bounded like `http.MaxBytesReader`.
    pub async fn read_all(&mut self, limit: Option<u64>) -> Result<Vec<u8>, BodyError> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = self.read(&mut buf).await?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
            if let Some(max) = limit {
                if out.len() as u64 > max {
                    return Err(BodyError::TooLarge);
                }
            }
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, BodyError> {
        if self.done || self.failed {
            return Ok(0);
        }
        if self.continue_pending {
            self.continue_pending = false;
            if self
                .writer
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .is_err()
            {
                self.failed = true;
                return Err(eof());
            }
        }
        self.read_any = true;
        let result = match self.framing {
            Framing::None => {
                self.done = true;
                Ok(0)
            }
            Framing::Length(_) => {
                if self.remaining == 0 {
                    self.done = true;
                    return Ok(0);
                }
                let want = buf.len().min(self.remaining as usize);
                match self.reader.read(&mut buf[..want]).await {
                    Ok(0) | Err(_) => Err(eof()),
                    Ok(n) => {
                        self.remaining -= n as u64;
                        Ok(n)
                    }
                }
            }
            Framing::Chunked => self.read_chunked(buf).await,
        };
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// `internal/chunked.go` reader, with its error texts.
    async fn read_chunked(&mut self, buf: &mut [u8]) -> Result<usize, BodyError> {
        if self.chunk_left == 0 {
            let line = read_chunk_line(&mut self.reader).await?;
            let line = line.trim_end_matches([' ', '\t']);
            let line = line.split(';').next().unwrap_or("");
            let size = parse_hex_uint(line)?;
            if size == 0 {
                // Trailer section: a bare CRLF, or header lines up to one.
                loop {
                    let t = read_line(&mut self.reader, 4096).await.map_err(|_| eof())?;
                    if t.is_empty() {
                        break;
                    }
                }
                self.done = true;
                return Ok(0);
            }
            self.chunk_left = size;
        }
        let want = buf.len().min(self.chunk_left as usize);
        let n = match self.reader.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => return Err(eof()),
            Ok(n) => n,
        };
        self.chunk_left -= n as u64;
        if self.chunk_left == 0 {
            let mut crlf = [0u8; 2];
            if self.reader.read_exact(&mut crlf).await.is_err() {
                return Err(eof());
            }
            if &crlf != b"\r\n" {
                return Err(BodyError::Io("malformed chunked encoding".into()));
            }
        }
        Ok(n)
    }

    /// After the handler: Go discards up to 256 KiB of unread body so the
    /// connection can be reused, and closes it otherwise.
    async fn finish(&mut self) -> bool {
        if self.done {
            return true;
        }
        if self.failed {
            return false;
        }
        if self.continue_pending && !self.read_any {
            // The client never got 100 Continue; Go closes after replying.
            return false;
        }
        let mut discarded = 0u64;
        let mut buf = vec![0u8; 16 * 1024];
        while discarded <= MAX_POST_HANDLER_READ_BYTES {
            match self.read(&mut buf).await {
                Ok(0) => return true,
                Ok(n) => discarded += n as u64,
                Err(_) => return false,
            }
        }
        false
    }
}

/// `readChunkLine`: CRLF-terminated, under 4096 bytes.
async fn read_chunk_line(reader: &mut BufReader<OwnedReadHalf>) -> Result<String, BodyError> {
    let mut raw = Vec::new();
    let mut limited = (&mut *reader).take(4096);
    match limited.read_until(b'\n', &mut raw).await {
        Ok(_) if raw.last() == Some(&b'\n') => {}
        Ok(_) if raw.len() >= 4096 => return Err(BodyError::Io("header line too long".into())),
        _ => return Err(eof()),
    }
    match raw.iter().position(|c| *c == b'\r') {
        None => return Err(BodyError::Io("chunked line ends with bare LF".into())),
        Some(i) if i != raw.len() - 2 => {
            return Err(BodyError::Io("invalid CR in chunked line".into()))
        }
        _ => {}
    }
    raw.truncate(raw.len() - 2);
    if raw.len() >= 4096 {
        return Err(BodyError::Io("header line too long".into()));
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// `parseHexUint`.
fn parse_hex_uint(v: &str) -> Result<u64, BodyError> {
    if v.is_empty() {
        return Err(BodyError::Io("empty hex number for chunk length".into()));
    }
    let mut n: u64 = 0;
    for (i, c) in v.bytes().enumerate() {
        let d = hex_val(c).ok_or_else(|| BodyError::Io("invalid byte in chunk length".into()))?;
        if i == 16 {
            return Err(BodyError::Io("http chunk length too large".into()));
        }
        n = n << 4 | d as u64;
    }
    Ok(n)
}

enum LineError {
    Eof,
    TooLong,
    Io,
}

async fn read_line(
    reader: &mut BufReader<OwnedReadHalf>,
    limit: usize,
) -> Result<String, LineError> {
    let mut raw = Vec::new();
    let mut limited = (&mut *reader).take(limit as u64 + 1);
    match limited.read_until(b'\n', &mut raw).await {
        Ok(0) => return Err(LineError::Eof),
        Ok(_) => {}
        Err(_) => return Err(LineError::Io),
    }
    if raw.last() != Some(&b'\n') {
        return Err(if raw.len() > limit {
            LineError::TooLong
        } else {
            LineError::Eof
        });
    }
    raw.pop();
    if raw.last() == Some(&b'\r') {
        raw.pop();
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// What went wrong while reading a request head, mapped to Go's replies.
enum ReadError {
    /// Connection closed or unreadable: no reply.
    Silent,
    TooLarge,
    UnsupportedTe,
    Status(u16, &'static str),
    Generic,
}

/// Head reader that enforces Go's initial read limit across all lines.
struct HeadReader<'a> {
    reader: &'a mut BufReader<OwnedReadHalf>,
    budget: usize,
}

impl HeadReader<'_> {
    async fn line(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut raw = Vec::new();
        let mut limited = (&mut *self.reader).take(self.budget as u64);
        let n = limited
            .read_until(b'\n', &mut raw)
            .await
            .map_err(|_| ReadError::Silent)?;
        self.budget = self.budget.saturating_sub(n);
        if raw.last() != Some(&b'\n') {
            return Err(if self.budget == 0 {
                ReadError::TooLarge
            } else {
                ReadError::Silent
            });
        }
        raw.pop();
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        Ok(raw)
    }

    async fn peek_is_fold(&mut self) -> bool {
        match self.reader.fill_buf().await {
            Ok(buf) => matches!(buf.first(), Some(b' ') | Some(b'\t')),
            Err(_) => false,
        }
    }
}

fn trim_ws(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| *c != b' ' && *c != b'\t')
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| *c != b' ' && *c != b'\t')
        .map_or(start, |i| i + 1);
    &b[start..end.max(start)]
}

async fn read_request(
    reader: &mut BufReader<OwnedReadHalf>,
    last_method_post: bool,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
) -> Result<Request, ReadError> {
    if last_method_post {
        // RFC 7230 section 3 tolerance for old buggy clients.
        for _ in 0..4 {
            match reader.fill_buf().await {
                Ok(buf) if matches!(buf.first(), Some(b'\r') | Some(b'\n')) => reader.consume(1),
                _ => break,
            }
        }
    }
    let mut head = HeadReader {
        reader,
        budget: MAX_HEADER_BYTES,
    };
    let first = head.line().await?;
    let line = String::from_utf8_lossy(&first).into_owned();
    let (method, rest) = line.split_once(' ').ok_or(ReadError::Generic)?;
    let (request_uri, proto) = rest.split_once(' ').ok_or(ReadError::Generic)?;
    if method.is_empty() || !method.bytes().all(valid_header_field_byte) {
        return Err(ReadError::Generic);
    }
    let (major, minor) = parse_http_version(proto).ok_or(ReadError::Generic)?;
    let url = if method == "CONNECT" && !request_uri.starts_with('/') {
        // The authority form: parsed as "http://" + authority, scheme dropped.
        parse_request_uri(&format!("http://{request_uri}")).ok_or(ReadError::Generic)?
    } else {
        parse_request_uri(request_uri).ok_or(ReadError::Generic)?
    };

    // MIME header block.
    let mut headers = Headers::default();
    if head.peek_is_fold().await {
        return Err(ReadError::Generic);
    }
    loop {
        let mut kv = head.line().await?;
        if kv.is_empty() {
            break;
        }
        while head.peek_is_fold().await {
            let cont = head.line().await?;
            kv = [trim_ws(&kv), b" ", trim_ws(&cont)].concat();
        }
        let kv = trim_ws(&kv).to_vec();
        let colon = kv
            .iter()
            .position(|c| *c == b':')
            .ok_or(ReadError::Generic)?;
        let (k, v) = (&kv[..colon], &kv[colon + 1..]);
        if k.is_empty() || !k.iter().all(|c| valid_header_field_byte(*c) || *c == b' ') {
            return Err(ReadError::Generic);
        }
        if !v.iter().all(|c| valid_header_value_byte(*c)) {
            return Err(ReadError::Generic);
        }
        let key_text = String::from_utf8_lossy(k).into_owned();
        let key = if k.contains(&b' ') {
            key_text
        } else {
            canonical_key(&key_text)
        };
        let value = String::from_utf8_lossy(trim_ws(v)).into_owned();
        headers.add_raw(key, value);
    }
    if headers.values("Host").len() > 1 {
        return Err(ReadError::Generic);
    }
    let host = if url.host.is_empty() {
        headers.get("Host").to_string()
    } else {
        url.host.clone()
    };

    // readTransfer: Transfer-Encoding, then Content-Length.
    let mut chunked = false;
    if (major, minor) >= (1, 1) && headers.has("Transfer-Encoding") {
        let te = headers.values("Transfer-Encoding").to_vec();
        if te.len() != 1 || !te[0].eq_ignore_ascii_case("chunked") {
            return Err(ReadError::UnsupportedTe);
        }
        chunked = true;
    }
    headers.del("Transfer-Encoding");
    let lens: Vec<String> = headers.values("Content-Length").to_vec();
    let mut length: Option<u64> = None;
    if !lens.is_empty() {
        let first = lens[0].trim_matches([' ', '\t']).to_string();
        if lens.iter().any(|l| l.trim_matches([' ', '\t']) != first) {
            return Err(ReadError::Generic);
        }
        if first.is_empty() || !first.bytes().all(|c| c.is_ascii_digit()) {
            return Err(ReadError::Generic);
        }
        length = Some(first.parse::<u64>().map_err(|_| ReadError::Generic)?);
        if length.unwrap() >= 1 << 63 {
            return Err(ReadError::Generic);
        }
        headers.set("Content-Length", first);
    }
    let close_semantics = should_close(major, minor, &headers);
    let framing = if chunked {
        headers.del("Content-Length");
        Framing::Chunked
    } else {
        match length {
            Some(0) | None => Framing::None,
            Some(n) => Framing::Length(n),
        }
    };
    // HTTP/1.0 request without a length and with close semantics reads to EOF
    // only for responses; requests default to no body (fixLength).
    let _ = close_semantics;

    // Server-level checks (conn.readRequest).
    if !(major == 1 || (major == 2 && minor == 0 && method == "PRI" && request_uri == "*")) {
        return Err(ReadError::Status(505, "unsupported protocol version"));
    }
    let hosts = headers.values("Host").to_vec();
    if (major, minor) >= (1, 1) && hosts.is_empty() && method != "CONNECT" {
        return Err(ReadError::Status(400, "missing required Host header"));
    }
    if hosts.len() == 1 && !hosts[0].bytes().all(valid_host_byte) {
        return Err(ReadError::Status(400, "malformed Host header"));
    }
    for (k, vv) in headers.iter() {
        if !k.bytes().all(valid_header_field_byte) {
            return Err(ReadError::Status(400, "invalid header name"));
        }
        if vv
            .iter()
            .any(|v| !v.bytes().all(|c| c == b'\t' || !(c < 0x20 || c == 0x7f)))
        {
            return Err(ReadError::Status(400, "invalid header value"));
        }
    }
    headers.del("Host");
    let expect = headers.get("Expect").to_string();
    Ok(Request {
        method: method.to_string(),
        request_uri: request_uri.to_string(),
        escaped_path: url.escaped_path,
        raw_query: url.raw_query,
        proto_major: major,
        proto_minor: minor,
        headers,
        host,
        local_addr,
        remote_addr,
        framing,
        expects_continue: expect.eq_ignore_ascii_case("100-continue"),
    })
}

fn should_close(major: u8, minor: u8, headers: &Headers) -> bool {
    if major < 1 {
        return true;
    }
    let has = |token: &str| {
        headers.values("Connection").iter().any(|v| {
            v.split(',')
                .any(|t| t.trim_matches([' ', '\t']).eq_ignore_ascii_case(token))
        })
    };
    if major == 1 && minor == 0 {
        return has("close") || !has("keep-alive");
    }
    has("close")
}

fn parse_http_version(vers: &str) -> Option<(u8, u8)> {
    match vers {
        "HTTP/1.1" => return Some((1, 1)),
        "HTTP/1.0" => return Some((1, 0)),
        _ => {}
    }
    let b = vers.as_bytes();
    if !vers.starts_with("HTTP/") || b.len() != 8 || b[6] != b'.' {
        return None;
    }
    if !b[5].is_ascii_digit() || !b[7].is_ascii_digit() {
        return None;
    }
    Some((b[5] - b'0', b[7] - b'0'))
}

struct ParsedUrl {
    host: String,
    escaped_path: String,
    raw_query: String,
}

/// `url.ParseRequestURI` for the forms a server receives.
fn parse_request_uri(raw: &str) -> Option<ParsedUrl> {
    if raw.bytes().any(|c| c < 0x20 || c == 0x7f) {
        return None;
    }
    if raw == "*" {
        return Some(ParsedUrl {
            host: String::new(),
            escaped_path: "*".into(),
            raw_query: String::new(),
        });
    }
    let (rest, query) = match raw.split_once('?') {
        Some((r, q)) => (r, q.to_string()),
        None => (raw, String::new()),
    };
    let (host, path_part) = if rest.starts_with('/') {
        (String::new(), rest.to_string())
    } else {
        // Absolute form: scheme "://" authority path.
        let colon = rest.find(':')?;
        let scheme = &rest[..colon];
        if scheme.is_empty()
            || !scheme.as_bytes()[0].is_ascii_alphabetic()
            || !scheme
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
        {
            return None;
        }
        let after = &rest[colon + 1..];
        let auth_rest = after.strip_prefix("//")?;
        let (authority, path) = match auth_rest.find('/') {
            Some(i) => (&auth_rest[..i], auth_rest[i..].to_string()),
            None => (auth_rest, String::new()),
        };
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if !authority.bytes().all(valid_host_byte) {
            return None;
        }
        (unescape(authority, false)?, path)
    };
    let path = unescape(&path_part, false)?;
    let escaped_path = if valid_encoded_path(&path_part)
        && unescape(&path_part, false).as_deref() == Some(path.as_str())
    {
        path_part.clone()
    } else {
        escape_path(&path)
    };
    Some(ParsedUrl {
        host,
        escaped_path,
        raw_query: query,
    })
}

fn valid_encoded_path(s: &str) -> bool {
    s.bytes().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
                    | b'%'
                    | b'['
                    | b']'
            )
    })
}

/// `url.PathUnescape`-style decoding (no '+' handling in paths).
fn unescape(s: &str, plus_is_space: bool) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if i + 2 >= b.len() {
                    return None;
                }
                let hi = hex_val(*b.get(i + 1)?)?;
                let lo = hex_val(*b.get(i + 2)?)?;
                out.push(hi << 4 | lo);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `url.URL{Path: p}.EscapedPath()` / `escape(p, encodePath)`.
pub fn escape_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for c in p.bytes() {
        let keep = c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// `url.ParseQuery(raw).Get(key)`: first value, '+' as space; malformed
/// pairs are skipped like Go does.
pub fn form_get(raw: &str, key: &str) -> String {
    for pair in raw.split('&') {
        if pair.is_empty() || pair.contains(';') {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (unescape(k, true), unescape(v, true)) else {
            continue;
        };
        if k == key {
            return v;
        }
    }
    String::new()
}

fn valid_header_field_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn valid_header_value_byte(c: u8) -> bool {
    c >= 0x80 || c == b'\t' || (0x20..0x7f).contains(&c)
}

fn valid_host_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'!' | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b':'
                | b';'
                | b'='
                | b'['
                | b']'
                | b'_'
                | b'~'
        )
}

/// `path.Clean` plus ServeMux's trailing-slash rule (`cleanPath`).
pub fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return "/".into();
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut np = format!("/{}", parts.join("/"));
    if p.ends_with('/') && np != "/" {
        np.push('/');
    }
    np
}

/// Segment-wise match of an escaped path against an exact pattern, the way
/// the go1.22+ routing tree compares literal segments (each segment is
/// unescaped, so `%6D` matches `m` but `%2F` never splits a segment).
pub fn path_matches(escaped: &str, pattern: &str) -> bool {
    let a: Vec<Option<String>> = escaped
        .split('/')
        .skip(1)
        .map(|s| unescape(s, false))
        .collect();
    let b: Vec<&str> = pattern.split('/').skip(1).collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.as_deref() == Some(*y))
}

pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Request Entity Too Large",
        415 => "Unsupported Media Type",
        417 => "Expectation Failed",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

fn body_allowed_for_status(code: u16) -> bool {
    !((100..200).contains(&code) || code == 204 || code == 304)
}

/// `http.TimeFormat` for the current time.
fn http_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::app::civil_from_days(days);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m - 1) as usize];
    format!(
        "{weekday}, {d:02} {month} {y} {:02}:{:02}:{:02} GMT",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Go's `bufferBeforeChunkingSize`: a handler body larger than this is sent
/// before the handler finishes, so it carries no Content-Length.
const BUFFER_BEFORE_CHUNKING: usize = 2048;

fn has_token(v: &str, token: &str) -> bool {
    v.split([',', ' ', '\t'])
        .any(|t| t.eq_ignore_ascii_case(token))
}

/// Serialize a complete response the way `chunkWriter.writeHeader` and
/// `finishRequest` do. Returns the bytes and whether to close afterwards.
fn encode_response(req: &RequestSummary, mut res: Response, mut close: bool) -> (Vec<u8>, bool) {
    let is_head = req.method == "HEAD";
    let is11 = req.proto_major > 1 || (req.proto_major == 1 && req.proto_minor >= 1);
    let allowed = body_allowed_for_status(res.status);
    let h = &mut res.headers;
    let (mut x_date, mut x_len, mut x_type, mut x_conn, mut x_te) = (
        None::<String>,
        None::<String>,
        None::<String>,
        None::<String>,
        None::<String>,
    );
    let handler_done = res.body.len() <= BUFFER_BEFORE_CHUNKING;
    let te = h.get("Transfer-Encoding").to_string();
    let has_te = !te.is_empty();
    if handler_done
        && !has_te
        && allowed
        && !h.has("Content-Length")
        && (!is_head || !res.body.is_empty())
    {
        x_len = Some(res.body.len().to_string());
    }
    let handler_cl = h.get("Content-Length").parse::<i64>().ok();
    if req.keepalive_10
        && !h.get("Content-Length").is_empty()
        && h.get("Connection") == "keep-alive"
    {
        close = false;
    }
    let mut has_cl = x_len.is_some() || handler_cl.is_some();
    if req.keepalive_10 && (is_head || has_cl || !allowed) {
        if !h.has("Connection") {
            x_conn = Some("keep-alive".into());
        }
    } else if !is11 || req.wants_close {
        close = true;
    }
    if h.get("Connection") == "close" {
        close = true;
    }
    if allowed {
        if !h.has("Content-Type")
            && h.get("Content-Encoding").is_empty()
            && !has_te
            && !res.body.is_empty()
        {
            x_type = Some(sniff(&res.body).into());
        }
    } else {
        h.del("Content-Length");
        h.del("Transfer-Encoding");
        if res.status == 304 {
            h.del("Content-Type");
        }
    }
    if !h.has("Date") {
        x_date = Some(http_date());
    }
    if has_cl && has_te && te != "identity" {
        h.del("Content-Length");
        has_cl = false;
    }
    let mut chunking = false;
    if is_head || !allowed || res.status == 204 || has_cl {
        h.del("Transfer-Encoding");
    } else if is11 {
        if has_te && te == "identity" {
            close = true;
            h.del("Transfer-Encoding");
        } else {
            chunking = true;
            x_te = Some("chunked".into());
            if has_te && te == "chunked" {
                h.del("Transfer-Encoding");
            }
        }
    } else {
        close = true;
        h.del("Transfer-Encoding");
    }
    if chunking {
        h.del("Content-Length");
    }
    if close && !has_token(h.get("Connection"), "close") {
        h.del("Connection");
        if is11 {
            x_conn = Some("close".into());
        }
    }
    let text = status_text(res.status);
    let text = if text.is_empty() {
        format!("status code {}", res.status)
    } else {
        text.to_string()
    };
    let proto = if is11 { "HTTP/1.1" } else { "HTTP/1.0" };
    let mut out = format!("{proto} {} {text}\r\n", res.status).into_bytes();
    let mut keys: Vec<(&str, &[String])> = h.iter().collect();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    for (k, vv) in keys {
        for v in vv {
            let v = v.replace(['\r', '\n'], " ");
            out.extend_from_slice(format!("{k}: {}\r\n", v.trim()).as_bytes());
        }
    }
    for (k, v) in [
        ("Date", x_date),
        ("Content-Length", x_len),
        ("Content-Type", x_type),
        ("Connection", x_conn),
        ("Transfer-Encoding", x_te),
    ] {
        if let Some(v) = v {
            out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
    }
    out.extend_from_slice(b"\r\n");
    if !is_head && allowed {
        if chunking {
            if !res.body.is_empty() {
                out.extend_from_slice(format!("{:x}\r\n", res.body.len()).as_bytes());
                out.extend_from_slice(&res.body);
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"0\r\n\r\n");
        } else {
            out.extend_from_slice(&res.body);
        }
    }
    (out, close)
}

/// The subset of `http.DetectContentType` that handler bodies here can hit.
fn sniff(body: &[u8]) -> &'static str {
    let trimmed: &[u8] = {
        let i = body
            .iter()
            .position(|c| !matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
            .unwrap_or(body.len());
        &body[i..]
    };
    if trimmed.starts_with(b"<") {
        return "text/html; charset=utf-8";
    }
    if body
        .iter()
        .any(|c| matches!(c, 0x00..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f))
    {
        return "application/octet-stream";
    }
    "text/plain; charset=utf-8"
}

fn wants_10_keepalive(req: &Request) -> bool {
    if req.proto_major != 1 || req.proto_minor != 0 {
        return false;
    }
    req.headers.values("Connection").iter().any(|v| {
        v.split(',').any(|t| {
            t.trim_matches([' ', '\t'])
                .eq_ignore_ascii_case("keep-alive")
        })
    })
}

/// Serve one connection until it closes. `handler` gets the request and its
/// body reader, and hands the reader back with a complete response so the
/// connection can apply Go's post-handler body rules.
pub async fn serve_conn<H, F>(stream: TcpStream, handler: H)
where
    H: Fn(Request, Body) -> F,
    F: Future<Output = (Response, Body)>,
{
    let (Ok(local), Ok(remote)) = (stream.local_addr(), stream.peer_addr()) else {
        return;
    };
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(4096, read_half);
    let mut writer = write_half;
    let mut last_post = false;
    loop {
        let req = match read_request(&mut reader, last_post, local, remote).await {
            Ok(req) => req,
            Err(err) => {
                let reply = match err {
                    ReadError::Silent => return,
                    ReadError::TooLarge => error_reply("431 Request Header Fields Too Large"),
                    ReadError::UnsupportedTe => {
                        "HTTP/1.1 501 Not Implemented\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nUnsupported transfer encoding".to_string()
                    }
                    ReadError::Status(code, text) => {
                        error_reply(&format!("{code} {}: {text}", status_text(code)))
                    }
                    ReadError::Generic => error_reply("400 Bad Request"),
                };
                let _ = writer.write_all(reply.as_bytes()).await;
                let _ = writer.shutdown().await;
                return;
            }
        };
        last_post = req.method == "POST";
        if !req.expects_continue && !req.headers.get("Expect").is_empty() {
            let _ = writer
                .write_all(b"HTTP/1.1 417 Expectation Failed\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
        let framing = req.framing;
        let summary = RequestSummary {
            method: req.method.clone(),
            proto_major: req.proto_major,
            proto_minor: req.proto_minor,
            keepalive_10: wants_10_keepalive(&req),
            wants_close: req
                .headers
                .values("Connection")
                .iter()
                .any(|v| has_token(v, "close")),
        };
        let expects_continue =
            req.expects_continue && req.proto_at_least(1, 1) && framing != Framing::None;
        let body = Body {
            reader,
            writer,
            framing,
            remaining: match framing {
                Framing::Length(n) => n,
                _ => 0,
            },
            chunk_left: 0,
            done: framing == Framing::None,
            continue_pending: req.expects_continue
                && req.proto_at_least(1, 1)
                && framing != Framing::None,
            failed: false,
            read_any: false,
        };
        let (mut response, mut body) = handler(req, body).await;
        // closeAfterReply as computed before the unread-body check.
        let mut close = (summary.proto_major, summary.proto_minor) < (1, 1)
            && !summary.keepalive_10
            || summary.wants_close
            || response.headers.get("Connection") == "close";
        if expects_continue && !body.done {
            close = true;
        }
        if !close && framing != Framing::None && !body.finish().await {
            // More than 256 KiB left unread: Go replies and closes.
            close = true;
            response.headers.del("Connection");
            response.headers.set("Connection", "close");
        }
        let (bytes, close) = encode_response(&summary, response, close);
        reader = body.reader;
        writer = body.writer;
        if writer.write_all(&bytes).await.is_err() || close {
            let _ = writer.shutdown().await;
            return;
        }
    }
}

/// What response encoding needs to know about the request.
struct RequestSummary {
    method: String,
    proto_major: u8,
    proto_minor: u8,
    keepalive_10: bool,
    wants_close: bool,
}

fn error_reply(public: &str) -> String {
    format!("HTTP/1.1 {public}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{public}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_matches_go() {
        assert_eq!(clean_path("//mcp"), "/mcp");
        assert_eq!(clean_path("/./mcp"), "/mcp");
        assert_eq!(clean_path("/x/../mcp"), "/mcp");
        assert_eq!(clean_path("/mcp/"), "/mcp/");
        assert_eq!(clean_path(""), "/");
        assert_eq!(clean_path("/.."), "/");
    }

    #[test]
    fn segment_matching_unescapes_each_segment() {
        assert!(path_matches("/%6Dcp", "/mcp"));
        assert!(!path_matches("/mcp%2F", "/mcp"));
        assert!(!path_matches("/MCP", "/mcp"));
        assert!(path_matches(
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource/mcp"
        ));
    }

    #[test]
    fn version_and_key_rules_match_go() {
        assert_eq!(parse_http_version("HTTP/9.9"), Some((9, 9)));
        assert_eq!(parse_http_version("HTTP/1.10"), None);
        assert_eq!(
            canonical_key("mcp-protocol-version"),
            "Mcp-Protocol-Version"
        );
        assert_eq!(canonical_key("x y"), "x y");
        assert_eq!(form_get("a=1&token=x+y%21", "token"), "x y!");
    }
}
