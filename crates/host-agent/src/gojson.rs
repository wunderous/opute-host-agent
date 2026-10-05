//! JSON decoding with Go `encoding/json` semantics where the wire contract
//! depends on them.
//!
//! The Go agent decodes the JSON-RPC envelope with `json.Unmarshal` into a
//! struct, which differs from a typical JSON library in ways a client can
//! observe:
//!
//! * object keys match struct fields case-insensitively, and the last
//!   matching key wins (`{"method":"a","METHOD":"b"}` decodes as "b");
//! * nesting up to 10000 levels is accepted;
//! * invalid UTF-8 inside strings is accepted and replaced with U+FFFD;
//! * a byte-order mark, trailing data, or a second value is a syntax error;
//! * `null` decodes into any field as "leave unset".
//!
//! The parser keeps object members in document order (duplicates included)
//! and the raw bytes of every value, which is what `json.RawMessage` holds.

const MAX_DEPTH: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The literal number text, exactly as written.
    Number(String),
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: Value,
    /// Raw bytes of this value (no surrounding whitespace).
    pub raw: Vec<u8>,
    /// Byte offset of the value in the parsed document.
    pub offset: usize,
}

impl Node {
    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Node)]> {
        match &self.value {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// Struct-field lookup with Go's rule: the last key that equals `name`
    /// under Unicode simple case folding wins.
    pub fn field(&self, name: &str) -> Option<&Node> {
        self.as_object()?
            .iter()
            .rev()
            .find(|(k, _)| equal_fold(k, name))
            .map(|(_, v)| v)
    }

    /// `map[string]json.RawMessage` lookup: exact key, last occurrence wins.
    pub fn key(&self, name: &str) -> Option<&Node> {
        self.as_object()?
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }
}

/// `strings.EqualFold`-compatible comparison for JSON keys (Go also folds
/// the Kelvin sign and long s, which matter for ASCII field names).
pub fn equal_fold(a: &str, b: &str) -> bool {
    let fold = |c: char| match c {
        '\u{212A}' => 'k',
        '\u{017F}' => 's',
        c => c.to_lowercase().next().unwrap_or(c),
    };
    let mut x = a.chars().map(fold);
    let mut y = b.chars().map(fold);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if p == q => continue,
            _ => return false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct SyntaxError;

/// Parse a complete JSON document. Leading/trailing whitespace is allowed;
/// anything else after the value is an error, as in `json.Unmarshal`.
pub fn parse(input: &[u8]) -> Result<Node, SyntaxError> {
    let mut p = Parser { b: input, i: 0 };
    p.ws();
    let node = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(SyntaxError);
    }
    Ok(node)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        let start = self.i;
        let value = match self.b.get(self.i) {
            Some(b'{') => self.object(depth + 1)?,
            Some(b'[') => self.array(depth + 1)?,
            Some(b'"') => Value::String(self.string()?),
            Some(b't') => self.literal(b"true", Value::Bool(true))?,
            Some(b'f') => self.literal(b"false", Value::Bool(false))?,
            Some(b'n') => self.literal(b"null", Value::Null)?,
            Some(c) if *c == b'-' || c.is_ascii_digit() => Value::Number(self.number()?),
            _ => return Err(SyntaxError),
        };
        Ok(Node {
            value,
            raw: self.b[start..self.i].to_vec(),
            offset: start,
        })
    }

    fn literal(&mut self, word: &[u8], v: Value) -> Result<Value, SyntaxError> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(SyntaxError)
        }
    }

    fn number(&mut self) -> Result<String, SyntaxError> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(c) if c.is_ascii_digit() => {
                while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
            }
            _ => return Err(SyntaxError),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(SyntaxError);
            }
            while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        if matches!(self.b.get(self.i), Some(b'e') | Some(b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            if !self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(SyntaxError);
            }
            while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        Ok(String::from_utf8_lossy(&self.b[start..self.i]).into_owned())
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.i += 1; // opening quote
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or(SyntaxError)?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                b'\\' => {
                    self.i += 1;
                    let e = *self.b.get(self.i).ok_or(SyntaxError)?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(SyntaxError),
                    }
                }
                c if c < 0x20 => return Err(SyntaxError),
                c => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, SyntaxError> {
        let s = self.b.get(self.i..self.i + 4).ok_or(SyntaxError)?;
        let text = std::str::from_utf8(s).map_err(|_| SyntaxError)?;
        let v = u32::from_str_radix(text, 16).map_err(|_| SyntaxError)?;
        self.i += 4;
        Ok(v)
    }

    fn array(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        if depth > MAX_DEPTH {
            return Err(SyntaxError);
        }
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth)?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(SyntaxError),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        if depth > MAX_DEPTH {
            return Err(SyntaxError);
        }
        self.i += 1;
        let mut members = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(SyntaxError);
            }
            let key = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(SyntaxError);
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth)?;
            members.push((key, v));
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(SyntaxError),
            }
        }
    }
}

/// Go `json.Unmarshal` of a value into a `string` field: only a JSON string
/// (or null, which leaves the field unset) is accepted.
pub fn go_string(node: Option<&Node>) -> Result<String, ()> {
    match node.map(|n| &n.value) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(()),
    }
}

// --- encoding ----------------------------------------------------------------------

/// Appends `s` as a JSON string the way `encoding/json` does with HTML
/// escaping on (the default for `json.Marshal`): `<`, `>` and `&` become
/// `\u003c`, `\u003e`, `\u0026`; U+2028 and U+2029 are escaped; `\b` and
/// `\f` use their short forms (Go 1.22+); other controls use `\u00XX`.
pub fn encode_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Post-processes already-serialized JSON bytes (from `serde_json`, which
/// does not HTML-escape) to match `json.Marshal`'s default HTML-escaping:
/// `<`, `>` and `&` become `<`, `>`, `&`, and U+2028/U+2029
/// are escaped. Safe as a blind text substitution because these five
/// characters can only occur inside an already-quoted JSON string in valid
/// output -- never as a structural character -- so there is no quoting
/// context to track. Any canonical hash or byte-for-byte comparison against
/// a Go `json.Marshal` output that goes through `serde_json` instead of
/// `gojson::encode_string` needs this, or a string field containing one of
/// these characters silently changes the hash.
pub fn html_escape_json_bytes(bytes: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8(bytes).expect("serde_json output is valid UTF-8");
    text.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .into_bytes()
}

/// `encoding/json`'s float64 formatting: shortest round-trip digits, plain
/// notation for 1e-6 <= |f| < 1e21, otherwise exponent form with `e-7`
/// (not `e-07`) and `e+21`.
pub fn encode_float(f: f64, out: &mut String) {
    let abs = f.abs();
    if abs == 0.0 || (1e-6..1e21).contains(&abs) {
        out.push_str(&format!("{f}"));
        return;
    }
    let text = format!("{f:e}");
    match text.split_once('e') {
        Some((mantissa, exp)) if !exp.starts_with('-') => {
            out.push_str(mantissa);
            out.push_str("e+");
            out.push_str(&format!("{exp:0>2}"));
        }
        Some((mantissa, exp)) => {
            out.push_str(mantissa);
            out.push('e');
            out.push_str(exp);
        }
        None => out.push_str(&text),
    }
}

/// The Go type a JSON value was marshalled from, as far as member order is
/// concerned: struct fields marshal in declaration order, map keys sorted.
pub enum Shape {
    /// `any` or a type with no nested structs: map semantics throughout.
    Any,
    /// A struct: fields in this order (absent fields were omitted).
    Struct(&'static [(&'static str, Shape)]),
    /// A map whose values all have one shape.
    Map(&'static Shape),
    /// A `map[string]any` holding some struct-typed values.
    Keys(&'static [(&'static str, Shape)]),
    /// A slice.
    List(&'static Shape),
}

/// `json.Marshal` of `value` as the Go type `shape` describes.
pub fn encode_shaped(value: &serde_json::Value, shape: &Shape, out: &mut String) {
    use serde_json::Value as J;
    let member = |out: &mut String, first: &mut bool, k: &str, v: &J, s: &Shape| {
        if !*first {
            out.push(',');
        }
        *first = false;
        encode_string(k, out);
        out.push(':');
        encode_shaped(v, s, out);
    };
    match (shape, value) {
        (Shape::Struct(fields), J::Object(map)) => {
            out.push('{');
            let mut first = true;
            for (k, s) in fields.iter() {
                if let Some(v) = map.get(*k) {
                    member(out, &mut first, k, v, s);
                }
            }
            out.push('}');
        }
        (Shape::Map(s), J::Object(map)) => {
            out.push('{');
            let mut first = true;
            for (k, v) in map {
                member(out, &mut first, k, v, s);
            }
            out.push('}');
        }
        (Shape::Keys(fields), J::Object(map)) => {
            out.push('{');
            let mut first = true;
            for (k, v) in map {
                let s = fields
                    .iter()
                    .find(|(f, _)| f == k)
                    .map_or(&Shape::Any, |(_, s)| s);
                member(out, &mut first, k, v, s);
            }
            out.push('}');
        }
        (Shape::List(s), J::Array(items)) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_shaped(item, s, out);
            }
            out.push(']');
        }
        _ => encode(value, out),
    }
}

/// A float64 as a JSON value: integral values stay integers, so the value
/// encodes exactly as Go writes it.
pub fn float_value(f: f64) -> serde_json::Value {
    if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
        serde_json::Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

/// `json.Marshal` of a value decoded into `any`: object keys sorted (as Go
/// sorts map keys), numbers as float64 unless they are exact integers.
pub fn encode(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value as J;
    match v {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                encode_float(n.as_f64().unwrap_or(0.0), out);
            }
        }
        J::String(s) => encode_string(s, out),
        J::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode(item, out);
            }
            out.push(']');
        }
        J::Object(map) => {
            // serde_json's default map is ordered by key bytes, which is the
            // order encoding/json uses for map keys.
            out.push('{');
            for (i, (key, value)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_string(key, out);
                out.push(':');
                encode(value, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_fold_and_last_wins() {
        let n = parse(br#"{"method":"a","METHOD":"b"}"#).unwrap();
        assert_eq!(n.field("method").unwrap().as_str(), Some("b"));
        assert_eq!(n.key("method").unwrap().as_str(), Some("a"));
    }

    #[test]
    fn go_rejections() {
        assert!(parse(b"\xef\xbb\xbf{}").is_err());
        assert!(parse(b"{} x").is_err());
        assert!(parse(b"{}{}").is_err());
        assert!(parse(b"01").is_err());
        assert!(parse(b" {} ").is_ok());
        // The server parses on a 256 MiB stack; do the same here.
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(|| {
                let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
                assert!(parse(deep.as_bytes()).is_ok());
                let too_deep = format!("{}{}", "[".repeat(10_001), "]".repeat(10_001));
                assert!(parse(too_deep.as_bytes()).is_err());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn raw_bytes_are_preserved() {
        let n = parse(br#"{"params": {"a" : 1}}"#).unwrap();
        assert_eq!(n.key("params").unwrap().raw, br#"{"a" : 1}"#);
    }

    #[test]
    fn encodes_like_encoding_json() {
        let mut s = String::new();
        encode_string("a<b>&\u{2028}\u{8}\u{c}\u{1}\"\\\n", &mut s);
        assert_eq!(s, r#""a\u003cb\u003e\u0026\u2028\b\f\u0001\"\\\n""#);
        let f = |x: f64| {
            let mut s = String::new();
            encode_float(x, &mut s);
            s
        };
        assert_eq!(f(0.25), "0.25");
        assert_eq!(f(2.0), "2");
        assert_eq!(f(1e21), "1e+21");
        assert_eq!(f(1e-7), "1e-7");
        assert_eq!(f(123456789.0), "123456789");
        assert_eq!(f(-1.5e-10), "-1.5e-10");
        // Checked against go1.25 encoding/json.
        assert_eq!(f(1e20), "100000000000000000000");
        assert_eq!(f(0.000001), "0.000001");
        assert_eq!(f(5e-324), "5e-324");
        assert_eq!(f(f64::MAX), "1.7976931348623157e+308");
        let v: serde_json::Value =
            serde_json::from_str(r#"{"b":[1,2.5,null],"a":{"z":true,"y":"x"}}"#).unwrap();
        let mut s = String::new();
        encode(&v, &mut s);
        assert_eq!(s, r#"{"a":{"y":"x","z":true},"b":[1,2.5,null]}"#);
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let n = parse(b"\"a\xffb\"").unwrap();
        assert_eq!(n.as_str(), Some("a\u{FFFD}b"));
    }
}
