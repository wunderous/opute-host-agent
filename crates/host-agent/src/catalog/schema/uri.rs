// Copyright 2009 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// Adapted from Go 1.25.4 net/url and net/netip; see licenses/Go-BSD.txt.
//! URI parsing and reference composition pinned to Go 1.25 net/url.
//! Keep decoded components as bytes: percent decoding need not yield UTF-8.
use super::quote;

#[derive(Clone, Default)]
pub(super) struct Uri {
    pub scheme: String,
    pub opaque: String,
    pub user: Option<(Vec<u8>, Option<Vec<u8>>)>,
    pub host: Vec<u8>,
    pub path: Vec<u8>,
    pub raw_path: String,
    pub omit_host: bool,
    pub force_query: bool,
    pub query: String,
    pub fragment: Vec<u8>,
    pub raw_fragment: String,
}
#[derive(Clone, Copy)]
enum Mode {
    Path,
    Host,
    Zone,
    User,
    Fragment,
}
fn should_escape(byte: u8, mode: Mode) -> bool {
    if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
        return false;
    }
    if matches!(mode, Mode::Host | Mode::Zone) && b"!$&'()*+,;=:[]<>\"".contains(&byte) {
        return false;
    }
    if b"$&+,/:;=?@".contains(&byte) {
        return match mode {
            Mode::Path => byte == b'?',
            Mode::User => b"@/?:".contains(&byte),
            Mode::Fragment => false,
            _ => true,
        };
    }
    !(matches!(mode, Mode::Fragment) && b"!()*".contains(&byte))
}
fn escape(bytes: &[u8], mode: Mode) -> String {
    let mut out = String::new();
    for &b in bytes {
        if should_escape(b, mode) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}
pub(super) fn quote_bytes(mut bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                let q = quote(text);
                out.push_str(&q[1..q.len() - 1]);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                let q = quote(std::str::from_utf8(&bytes[..good]).unwrap());
                out.push_str(&q[1..q.len() - 1]);
                out.push_str(&format!("\\x{:02x}", bytes[good]));
                bytes = &bytes[good + 1..];
            }
        }
    }
    out.push('"');
    out
}
fn unescape(raw: &str, mode: Mode) -> Result<Vec<u8>, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit()
            {
                return Err(format!(
                    "invalid URL escape {}",
                    quote_bytes(&bytes[i..bytes.len().min(i + 3)])
                ));
            }
            let v =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap();
            if matches!(mode, Mode::Host) && v < 128 && &bytes[i..i + 3] != b"%25" {
                return Err(format!(
                    "invalid URL escape {}",
                    quote_bytes(&bytes[i..i + 3])
                ));
            }
            if matches!(mode, Mode::Zone)
                && &bytes[i..i + 3] != b"%25"
                && v != b' '
                && should_escape(v, Mode::Host)
            {
                return Err(format!(
                    "invalid URL escape {}",
                    quote_bytes(&bytes[i..i + 3])
                ));
            }
            out.push(v);
            i += 3;
        } else {
            if matches!(mode, Mode::Host | Mode::Zone) && b < 128 && should_escape(b, mode) {
                return Err(format!(
                    "invalid character {} in host name",
                    quote_bytes(&[b])
                ));
            }
            out.push(b);
            i += 1;
        }
    }
    Ok(out)
}
fn valid_encoded(raw: &str, mode: Mode) -> bool {
    raw.bytes()
        .all(|b| b"!$&'()*+,;=:@[]%".contains(&b) || !should_escape(b, mode))
}
fn optional_port(port: &str) -> bool {
    port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
}
fn host(raw: &str) -> Result<Vec<u8>, String> {
    if let Some(open) = raw.rfind('[') {
        let close = raw.rfind(']').ok_or("missing ']' in host")?;
        let port = &raw[close + 1..];
        if !optional_port(port) {
            return Err(format!("invalid port {} after host", quote(port)));
        }
        let port = unescape(port, Mode::Host)?;
        let name = &raw[open + 1..close];
        let mut decoded = if let Some(zone) = name.find("%25") {
            let mut value = unescape(&name[..zone], Mode::Host)?;
            value.extend(unescape(&name[zone..], Mode::Zone)?);
            value
        } else {
            unescape(name, Mode::Host)?
        };
        if !ip_literal(&decoded)? {
            return Err("invalid IP-literal".into());
        }
        let mut result = vec![b'['];
        result.append(&mut decoded);
        result.push(b']');
        result.extend(port);
        return Ok(result);
    }
    if let Some(colon) = raw.rfind(':') {
        let port = &raw[colon..];
        if !optional_port(port) {
            return Err(format!("invalid port {} after host", quote(port)));
        }
    }
    unescape(raw, Mode::Host)
}
impl Uri {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (main, fragment) = raw.split_once('#').map_or((raw, ""), |(a, b)| (a, b));
        let mut uri = Self::parse_main(main).map_err(|e| format!("parse {}: {e}", quote(main)))?;
        if !fragment.is_empty() {
            uri.set_fragment(fragment)
                .map_err(|e| format!("parse {}: {e}", quote(raw)))?;
        }
        Ok(uri)
    }
    fn parse_main(raw: &str) -> Result<Self, String> {
        if raw.bytes().any(|b| b < 32 || b == 127) {
            return Err("net/url: invalid control character in URL".into());
        }
        let mut uri = Self::default();
        if raw == "*" {
            uri.path = vec![b'*'];
            return Ok(uri);
        }
        let mut rest = raw;
        for (i, b) in raw.bytes().enumerate() {
            if b.is_ascii_alphabetic() {
                continue;
            }
            if b.is_ascii_digit() || b"+-.".contains(&b) {
                if i == 0 {
                    break;
                }
                continue;
            }
            if b == b':' {
                if i == 0 {
                    return Err("missing protocol scheme".into());
                }
                uri.scheme = raw[..i].to_ascii_lowercase();
                rest = &raw[i + 1..];
            }
            break;
        }
        if rest.ends_with('?') && rest.bytes().filter(|&b| b == b'?').count() == 1 {
            uri.force_query = true;
            rest = &rest[..rest.len() - 1];
        } else if let Some((path, query)) = rest.split_once('?') {
            rest = path;
            uri.query = query.into();
        }
        if !rest.starts_with('/') {
            if !uri.scheme.is_empty() {
                uri.opaque = rest.into();
                return Ok(uri);
            }
            if rest.split('/').next().unwrap().contains(':') {
                return Err("first path segment in URL cannot contain colon".into());
            }
        }
        if rest.starts_with("//") && (!uri.scheme.is_empty() || !rest.starts_with("///")) {
            let (authority, path) = rest[2..]
                .split_once('/')
                .map_or((&rest[2..], ""), |(a, _)| (a, &rest[2 + a.len()..]));
            rest = path;
            let (userinfo, hostname) = authority
                .rsplit_once('@')
                .map_or((None, authority), |(a, b)| (Some(a), b));
            uri.host = host(hostname)?;
            if let Some(user) = userinfo {
                if !user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&b))
                {
                    return Err("net/url: invalid userinfo".into());
                }
                uri.user = Some(if let Some((name, password)) = user.split_once(':') {
                    (
                        unescape(name, Mode::User)?,
                        Some(unescape(password, Mode::User)?),
                    )
                } else {
                    (unescape(user, Mode::User)?, None)
                });
            }
        } else if !uri.scheme.is_empty() && rest.starts_with('/') {
            uri.omit_host = true;
        }
        uri.set_path(rest)?;
        Ok(uri)
    }
    fn set_path(&mut self, raw: &str) -> Result<(), String> {
        self.path = unescape(raw, Mode::Path)?;
        self.raw_path = if escape(&self.path, Mode::Path) == raw {
            String::new()
        } else {
            raw.into()
        };
        Ok(())
    }
    fn set_fragment(&mut self, raw: &str) -> Result<(), String> {
        self.fragment = unescape(raw, Mode::Fragment)?;
        self.raw_fragment = if escape(&self.fragment, Mode::Fragment) == raw {
            String::new()
        } else {
            raw.into()
        };
        Ok(())
    }
    pub fn escaped_path(&self) -> String {
        if !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path, Mode::Path)
            && unescape(&self.raw_path, Mode::Path).ok().as_ref() == Some(&self.path)
        {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            "*".into()
        } else {
            escape(&self.path, Mode::Path)
        }
    }
    pub fn escaped_fragment(&self) -> String {
        if !self.raw_fragment.is_empty()
            && valid_encoded(&self.raw_fragment, Mode::Fragment)
            && unescape(&self.raw_fragment, Mode::Fragment).ok().as_ref() == Some(&self.fragment)
        {
            self.raw_fragment.clone()
        } else {
            escape(&self.fragment, Mode::Fragment)
        }
    }
    pub fn string(&self) -> String {
        let mut out = String::new();
        if !self.scheme.is_empty() {
            out.push_str(&self.scheme);
            out.push(':');
        }
        if !self.opaque.is_empty() {
            out.push_str(&self.opaque);
        } else {
            if !self.host.is_empty()
                || self.user.is_some()
                || (!self.scheme.is_empty() && !self.omit_host)
            {
                if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                    out.push_str("//");
                }
                if let Some((name, password)) = &self.user {
                    out.push_str(&escape(name, Mode::User));
                    if let Some(p) = password {
                        out.push(':');
                        out.push_str(&escape(p, Mode::User));
                    }
                    out.push('@');
                }
                out.push_str(&escape(&self.host, Mode::Host));
            }
            let path = self.escaped_path();
            if !path.is_empty() && !path.starts_with('/') && !self.host.is_empty() {
                out.push('/');
            }
            if out.is_empty() && path.split('/').next().unwrap().contains(':') {
                out.push_str("./");
            }
            out.push_str(&path);
        }
        if self.force_query || !self.query.is_empty() {
            out.push('?');
            out.push_str(&self.query);
        }
        if !self.fragment.is_empty() {
            out.push('#');
            out.push_str(&self.escaped_fragment());
        }
        out
    }
    pub fn resolve(&self, reference: &Self) -> Self {
        let mut result = reference.clone();
        if reference.scheme.is_empty() {
            result.scheme = self.scheme.clone();
        }
        if !reference.scheme.is_empty() || !reference.host.is_empty() || reference.user.is_some() {
            result
                .set_path(&resolve_path(&reference.escaped_path(), ""))
                .unwrap();
            return result;
        }
        if !reference.opaque.is_empty() {
            result.user = None;
            result.host.clear();
            result.path.clear();
            return result;
        }
        if reference.path.is_empty() && !reference.force_query && reference.query.is_empty() {
            result.query = self.query.clone();
            if reference.fragment.is_empty() {
                result.fragment = self.fragment.clone();
                result.raw_fragment = self.raw_fragment.clone();
            }
        }
        if reference.path.is_empty() && !self.opaque.is_empty() {
            result.opaque = self.opaque.clone();
            result.user = None;
            result.host.clear();
            result.path.clear();
            return result;
        }
        result.host = self.host.clone();
        result.user = self.user.clone();
        result
            .set_path(&resolve_path(
                &self.escaped_path(),
                &reference.escaped_path(),
            ))
            .unwrap();
        result
    }
}
fn resolve_path(base: &str, reference: &str) -> String {
    let full = if reference.is_empty() {
        base.to_string()
    } else if reference.starts_with('/') {
        reference.into()
    } else {
        format!(
            "{}{reference}",
            &base[..base.rfind('/').map_or(0, |i| i + 1)]
        )
    };
    if full.is_empty() {
        return full;
    }
    let mut result = String::from("/");
    let mut first = true;
    let mut last = "";
    for segment in full.split('/') {
        last = segment;
        if segment == "." {
            first = false;
            continue;
        }
        if segment == ".." {
            let body = &result[1..];
            if let Some(index) = body.rfind('/') {
                result = format!("/{}", &body[..index]);
            } else {
                result = "/".into();
                first = true;
            }
        } else {
            if !first {
                result.push('/');
            }
            result.push_str(segment);
            first = false;
        }
    }
    if last == "." || last == ".." {
        result.push('/');
    }
    if result.len() > 1 && result.as_bytes()[1] == b'/' {
        result.remove(0);
    }
    result
}

// IP-literal validation returns the exact netip diagnostic, without choosing
// browser canonicalization or changing the original host representation.
fn ip_literal(input: &[u8]) -> Result<bool, String> {
    fn error(input: &[u8], message: &str, at: &[u8]) -> String {
        let mut out = format!("invalid host: ParseAddr({}): {message}", quote_bytes(input));
        if !at.is_empty() {
            out.push_str(&format!(" (at {})", quote_bytes(at)));
        }
        out
    }
    fn ipv4(input: &[u8], bytes: &[u8]) -> Result<(), String> {
        let mut fields = 0;
        let mut value = 0;
        let mut digits = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b.is_ascii_digit() {
                if digits == 1 && value == 0 {
                    return Err(error(input, "IPv4 field has octet with leading zero", &[]));
                }
                value = value * 10 + u32::from(b - b'0');
                if value > 255 {
                    return Err(error(input, "IPv4 field has value >255", &[]));
                }
                digits += 1;
            } else if b == b'.' {
                if digits == 0 || i + 1 == bytes.len() {
                    return Err(error(
                        input,
                        "IPv4 field must have at least one digit",
                        &bytes[i..],
                    ));
                }
                fields += 1;
                if fields > 3 {
                    return Err(error(input, "IPv4 address too long", &[]));
                }
                value = 0;
                digits = 0;
            } else {
                return Err(error(input, "unexpected character", &bytes[i..]));
            }
        }
        if fields != 3 || digits == 0 {
            return Err(error(input, "IPv4 address too short", &[]));
        }
        Ok(())
    }
    let first = input.iter().find(|&&b| b == b'.' || b == b':' || b == b'%');
    match first {
        Some(b'.') => {
            ipv4(input, input)?;
            return Ok(false);
        }
        Some(b'%') => return Err(error(input, "missing IPv6 address", &[])),
        None => return Err(error(input, "unable to parse IP", &[])),
        _ => (),
    }
    let mut rest = input;
    if let Some(zone) = input.iter().position(|&b| b == b'%') {
        if zone + 1 == input.len() {
            return Err(error(input, "zone must be a non-empty string", &[]));
        }
        rest = &input[..zone];
    }
    let mut ellipsis = false;
    let mut size = 0;
    if rest.starts_with(b"::") {
        ellipsis = true;
        rest = &rest[2..];
        if rest.is_empty() {
            return Ok(true);
        }
    }
    while size < 16 {
        let digits = rest.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if digits > 4 {
            return Err(error(input, "each group must have 4 or less digits", rest));
        }
        if digits == 0 {
            return Err(error(
                input,
                "each colon-separated field must have at least one digit",
                rest,
            ));
        }
        if rest.get(digits) == Some(&b'.') {
            if !ellipsis && size != 12 {
                return Err(error(
                    input,
                    "embedded IPv4 address must replace the final 2 fields of the address",
                    rest,
                ));
            }
            if size + 4 > 16 {
                return Err(error(
                    input,
                    "too many hex fields to fit an embedded IPv4 at the end of the address",
                    rest,
                ));
            }
            ipv4(input, rest)?;
            size += 4;
            rest = &[];
            break;
        }
        size += 2;
        rest = &rest[digits..];
        if rest.is_empty() {
            break;
        }
        if rest[0] != b':' {
            return Err(error(input, "unexpected character, want colon", rest));
        }
        if rest.len() == 1 {
            return Err(error(
                input,
                "colon must be followed by more characters",
                rest,
            ));
        }
        rest = &rest[1..];
        if rest[0] == b':' {
            if ellipsis {
                return Err(error(input, "multiple :: in address", rest));
            }
            ellipsis = true;
            rest = &rest[1..];
            if rest.is_empty() {
                break;
            }
        }
    }
    if !rest.is_empty() {
        return Err(error(input, "trailing garbage after address", rest));
    }
    if size < 16 && !ellipsis {
        return Err(error(input, "address string too short", &[]));
    }
    if size == 16 && ellipsis {
        return Err(error(
            input,
            "the :: must expand to at least one field of zeros",
            &[],
        ));
    }
    Ok(true)
}
