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

pub(crate) mod validate;
pub(crate) use validate::syntax as validate_syntax;

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
/// `json.Marshal` of a generic value, as a string.
pub fn encode_to_string(v: &serde_json::Value) -> String {
    let mut out = String::new();
    encode(v, &mut out);
    out
}

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

/// Validate Go standard base64, retaining corruption offsets and CR/LF rules.
pub(crate) fn validate_base64(bytes: &[u8]) -> Result<(), String> {
    let error = |i| format!("illegal base64 data at input byte {i}");
    let mut index = 0;
    loop {
        let mut digit = 0;
        while digit < 4 {
            if index == bytes.len() {
                return if digit == 0 {
                    Ok(())
                } else {
                    Err(error(index - digit))
                };
            }
            let value = bytes[index];
            index += 1;
            if value.is_ascii_alphanumeric() || matches!(value, b'+' | b'/') {
                digit += 1;
                continue;
            }
            if matches!(value, b'\r' | b'\n') {
                continue;
            }
            if value != b'=' || digit < 2 {
                return Err(error(index - 1));
            }
            if digit == 2 {
                while index < bytes.len() && matches!(bytes[index], b'\r' | b'\n') {
                    index += 1;
                }
                if index == bytes.len() {
                    return Err(error(bytes.len()));
                }
                if bytes[index] != b'=' {
                    return Err(error(index - 1));
                }
                index += 1;
            }
            while index < bytes.len() && matches!(bytes[index], b'\r' | b'\n') {
                index += 1;
            }
            return if index == bytes.len() {
                Ok(())
            } else {
                Err(error(index))
            };
        }
    }
}

// --- encoding/json scanner errors --------------------------------------------------

/// `quoteChar`: how the scanner names an offending byte.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => "'\\''".into(),
        b'"' => "'\"'".into(),
        _ => {
            let ch = char::from(c);
            let quoted = match ch {
                '\x07' => "\\a".to_string(),
                '\x08' => "\\b".to_string(),
                '\x0c' => "\\f".to_string(),
                '\n' => "\\n".to_string(),
                '\r' => "\\r".to_string(),
                '\t' => "\\t".to_string(),
                '\x0b' => "\\v".to_string(),
                '\\' => "\\\\".to_string(),
                c if (c as u32) < 0x20 || c == '\x7f' => format!("\\x{:02x}", c as u32),
                // U+0080..U+00A0 are not printable (U+00A0 is a non-ASCII
                // space); Go quotes them as \u.
                c if (0x80..=0xa0).contains(&(c as u32)) => format!("\\u{:04x}", c as u32),
                // U+00AD (soft hyphen) is not printable either.
                '\u{ad}' => "\\u00ad".to_string(),
                c => c.to_string(),
            };
            format!("'{quoted}'")
        }
    }
}

/// `json.Valid`'s first error, as `checkValid` reports it: Go's scanner,
/// state for state, so the message names the same byte and context.
pub fn syntax_error(data: &[u8]) -> Option<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Parse {
        ObjectKey,
        ObjectValue,
        ArrayValue,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        BeginValue,
        BeginValueOrEmpty,
        BeginStringOrEmpty,
        BeginString,
        EndValue,
        EndTop,
        InString,
        InStringEsc,
        InStringEscU(u8),
        Neg,
        One,
        Zero,
        Dot,
        Dot0,
        E,
        ESign,
        E0,
        Lit(&'static [u8], usize, &'static str),
    }
    let invalid =
        |c: u8, context: &str| Some(format!("invalid character {} {context}", quote_char(c)));
    let space = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n');
    let mut stack: Vec<Parse> = Vec::new();
    let mut st = St::BeginValue;
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        // Each arm either consumes `c` (i += 1) or re-dispatches it in a
        // new state, as Go's step functions do via stateEndValue.
        match st {
            St::BeginValueOrEmpty | St::BeginValue => {
                if space(c) {
                    i += 1;
                    continue;
                }
                if st == St::BeginValueOrEmpty && c == b']' {
                    stack.pop();
                    st = St::EndValue;
                    i += 1;
                    continue;
                }
                st = match c {
                    b'{' => {
                        if stack.len() >= 10_000 {
                            return Some("exceeded max depth".into());
                        }
                        stack.push(Parse::ObjectKey);
                        St::BeginStringOrEmpty
                    }
                    b'[' => {
                        if stack.len() >= 10_000 {
                            return Some("exceeded max depth".into());
                        }
                        stack.push(Parse::ArrayValue);
                        St::BeginValueOrEmpty
                    }
                    b'"' => St::InString,
                    b'-' => St::Neg,
                    b'0' => St::Zero,
                    b'1'..=b'9' => St::One,
                    b't' => St::Lit(b"rue", 0, "true"),
                    b'f' => St::Lit(b"alse", 0, "false"),
                    b'n' => St::Lit(b"ull", 0, "null"),
                    _ => return invalid(c, "looking for beginning of value"),
                };
                i += 1;
            }
            St::BeginStringOrEmpty | St::BeginString => {
                if space(c) {
                    i += 1;
                    continue;
                }
                if st == St::BeginStringOrEmpty && c == b'}' {
                    stack.pop();
                    st = St::EndValue;
                    i += 1;
                    continue;
                }
                if c != b'"' {
                    return invalid(c, "looking for beginning of object key string");
                }
                st = St::InString;
                i += 1;
            }
            St::EndValue => {
                let Some(&top) = stack.last() else {
                    st = St::EndTop;
                    continue;
                };
                if space(c) {
                    i += 1;
                    continue;
                }
                match (top, c) {
                    (Parse::ObjectKey, b':') => {
                        *stack.last_mut().expect("in an object") = Parse::ObjectValue;
                        st = St::BeginValue;
                    }
                    (Parse::ObjectKey, _) => return invalid(c, "after object key"),
                    (Parse::ObjectValue, b',') => {
                        *stack.last_mut().expect("in an object") = Parse::ObjectKey;
                        st = St::BeginString;
                    }
                    (Parse::ObjectValue, b'}') => {
                        stack.pop();
                        st = St::EndValue;
                    }
                    (Parse::ObjectValue, _) => return invalid(c, "after object key:value pair"),
                    (Parse::ArrayValue, b',') => st = St::BeginValue,
                    (Parse::ArrayValue, b']') => {
                        stack.pop();
                        st = St::EndValue;
                    }
                    (Parse::ArrayValue, _) => return invalid(c, "after array element"),
                }
                i += 1;
            }
            St::EndTop => {
                if !space(c) {
                    return invalid(c, "after top-level value");
                }
                i += 1;
            }
            St::InString => {
                match c {
                    b'"' => st = St::EndValue,
                    b'\\' => st = St::InStringEsc,
                    c if c < 0x20 => return invalid(c, "in string literal"),
                    _ => {}
                }
                i += 1;
            }
            St::InStringEsc => {
                st = match c {
                    b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => St::InString,
                    b'u' => St::InStringEscU(0),
                    _ => return invalid(c, "in string escape code"),
                };
                i += 1;
            }
            St::InStringEscU(n) => {
                if !c.is_ascii_hexdigit() {
                    return invalid(c, "in \\u hexadecimal character escape");
                }
                st = if n == 3 {
                    St::InString
                } else {
                    St::InStringEscU(n + 1)
                };
                i += 1;
            }
            St::Neg => {
                st = match c {
                    b'0' => St::Zero,
                    b'1'..=b'9' => St::One,
                    _ => return invalid(c, "in numeric literal"),
                };
                i += 1;
            }
            St::One | St::Zero => {
                if st == St::One && c.is_ascii_digit() {
                    i += 1;
                    continue;
                }
                match c {
                    b'.' => st = St::Dot,
                    b'e' | b'E' => st = St::E,
                    _ => {
                        st = St::EndValue;
                        continue;
                    }
                }
                i += 1;
            }
            St::Dot => {
                if !c.is_ascii_digit() {
                    return invalid(c, "after decimal point in numeric literal");
                }
                st = St::Dot0;
                i += 1;
            }
            St::Dot0 => {
                if c.is_ascii_digit() {
                    i += 1;
                    continue;
                }
                if c == b'e' || c == b'E' {
                    st = St::E;
                    i += 1;
                    continue;
                }
                st = St::EndValue;
            }
            St::E => {
                st = match c {
                    b'+' | b'-' => St::ESign,
                    b'0'..=b'9' => St::E0,
                    _ => return invalid(c, "in exponent of numeric literal"),
                };
                i += 1;
            }
            St::ESign => {
                if !c.is_ascii_digit() {
                    return invalid(c, "in exponent of numeric literal");
                }
                st = St::E0;
                i += 1;
            }
            St::E0 => {
                if c.is_ascii_digit() {
                    i += 1;
                    continue;
                }
                st = St::EndValue;
            }
            St::Lit(rest, k, word) => {
                if c != rest[k] {
                    return invalid(
                        c,
                        &format!("in literal {word} (expecting {})", quote_char(rest[k])),
                    );
                }
                st = if k + 1 == rest.len() {
                    St::EndValue
                } else {
                    St::Lit(rest, k + 1, word)
                };
                i += 1;
            }
        }
    }
    // `scanner.eof`: complete only after a whole top-level value.
    let complete = stack.is_empty()
        && matches!(
            st,
            St::EndValue | St::EndTop | St::One | St::Zero | St::Dot0 | St::E0
        );
    (!complete).then(|| "unexpected end of JSON input".to_string())
}

/// `json.Unmarshal(data, &[]struct{ Name string `json:"name"` }{})`: the
/// names, or Go's first syntax or type error.
pub fn unmarshal_names(data: &[u8]) -> Result<Vec<String>, String> {
    const ELEMENT: &str = "struct { Name string \"json:\\\"name\\\"\" }";
    if let Some(e) = syntax_error(data) {
        return Err(e);
    }
    let root = parse(data).map_err(|_| "unexpected end of JSON input".to_string())?;
    let kind = |v: &Value| match v {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Bool(_) => "bool",
        _ => "number",
    };
    let items = match &root.value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type []{ELEMENT}",
                kind(other)
            ))
        }
    };
    let mut first: Option<String> = None;
    let mut names = Vec::new();
    for item in items {
        let mut name = String::new();
        match &item.value {
            Value::Null => {}
            Value::Object(members) => {
                for (key, value) in members {
                    if !equal_fold(key, "name") {
                        continue;
                    }
                    match &value.value {
                        Value::Null => {}
                        Value::String(text) => name = text.clone(),
                        other => {
                            first.get_or_insert_with(|| {
                                format!(
                                    "json: cannot unmarshal {} into Go struct field .name of type string",
                                    kind(other)
                                )
                            });
                        }
                    }
                }
            }
            other => {
                first.get_or_insert_with(|| {
                    format!(
                        "json: cannot unmarshal {} into Go value of type {ELEMENT}",
                        kind(other)
                    )
                });
            }
        }
        names.push(name);
    }
    match first {
        Some(e) => Err(e),
        None => Ok(names),
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

    #[test]
    fn syntax_errors_match_go() {
        let e = |s: &str| syntax_error(s.as_bytes());
        assert_eq!(e("[]"), None);
        assert_eq!(e(" [1, 2.5e3, \"a\\u00e9\", true, null] "), None);
        assert_eq!(e("").as_deref(), Some("unexpected end of JSON input"));
        assert_eq!(e("[1,").as_deref(), Some("unexpected end of JSON input"));
        assert_eq!(
            e("x").as_deref(),
            Some("invalid character 'x' looking for beginning of value")
        );
        assert_eq!(
            e("[] x").as_deref(),
            Some("invalid character 'x' after top-level value")
        );
        assert_eq!(
            e("{\"a\" 1}").as_deref(),
            Some("invalid character '1' after object key")
        );
        assert_eq!(
            e("{\"a\":1 2}").as_deref(),
            Some("invalid character '2' after object key:value pair")
        );
        assert_eq!(
            e("[1 2]").as_deref(),
            Some("invalid character '2' after array element")
        );
        assert_eq!(
            e("{1}").as_deref(),
            Some("invalid character '1' looking for beginning of object key string")
        );
        assert_eq!(
            e("[tru]").as_deref(),
            Some("invalid character ']' in literal true (expecting 'e')")
        );
        assert_eq!(
            e("[-]").as_deref(),
            Some("invalid character ']' in numeric literal")
        );
        assert_eq!(
            e("[1.]").as_deref(),
            Some("invalid character ']' after decimal point in numeric literal")
        );
        assert_eq!(
            e("\"\n\"").as_deref(),
            Some("invalid character '\\n' in string literal")
        );
        assert_eq!(
            e("01").as_deref(),
            Some("invalid character '1' after top-level value")
        );
    }

    #[test]
    fn names_unmarshal_like_go() {
        assert_eq!(
            unmarshal_names(br#"[{"name":"a"},{"NAME":"b"},null,{}]"#).unwrap(),
            ["a", "b", "", ""]
        );
        assert_eq!(unmarshal_names(b"null").unwrap(), Vec::<String>::new());
        assert_eq!(
            unmarshal_names(b"{}").unwrap_err(),
            "json: cannot unmarshal object into Go value of type []struct { Name string \"json:\\\"name\\\"\" }"
        );
        assert_eq!(
            unmarshal_names(br#"[{"name":1}]"#).unwrap_err(),
            "json: cannot unmarshal number into Go struct field .name of type string"
        );
    }
}
