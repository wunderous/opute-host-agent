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
    fn invalid_utf8_is_replaced() {
        let n = parse(b"\"a\xffb\"").unwrap();
        assert_eq!(n.as_str(), Some("a\u{FFFD}b"));
    }
}
