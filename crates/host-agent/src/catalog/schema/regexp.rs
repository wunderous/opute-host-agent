// Copyright 2009 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause; see licenses/Go-BSD.txt.
//! Native Go 1.25 regexp syntax admission. Pattern execution is not part of
//! catalog schema compilation. This private parser is not yet promoted: AST
//! factoring, class expansion and compiler resource accounting remain incomplete.
use super::regexp_names::NAMES;

fn error(code: &str, expression: &str) -> String {
    format!("error parsing regexp: {code}: `{expression}`")
}
fn canonical(name: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for byte in name.bytes() {
        if b"_ -".contains(&byte) {
            continue;
        }
        let byte = if first {
            byte.to_ascii_uppercase()
        } else {
            byte.to_ascii_lowercase()
        };
        first = false;
        out.push(byte as char);
    }
    out
}
#[derive(Clone, Copy)]
struct Node {
    height: usize,
    copies: u64,
    size: u64,
    literal: bool,
    fold: bool,
}
impl Node {
    fn atom() -> Self {
        Self {
            height: 1,
            copies: 1,
            size: 1,
            literal: false,
            fold: false,
        }
    }
    fn literal(fold: bool) -> Self {
        Self {
            literal: true,
            fold,
            ..Self::atom()
        }
    }
}
struct Frame {
    flags: bool,
    capture: bool,
    branches: Vec<Node>,
    nodes: Vec<Node>,
}
impl Frame {
    fn new(flags: bool, capture: bool) -> Self {
        Self {
            flags,
            capture,
            branches: Vec::new(),
            nodes: Vec::new(),
        }
    }
    fn concat(&mut self) -> Node {
        let nodes = std::mem::take(&mut self.nodes);
        if nodes.len() == 1 {
            return nodes[0];
        }
        if nodes.is_empty() {
            return Node::atom();
        }
        Node {
            height: 1 + nodes.iter().map(|n| n.height).max().unwrap(),
            copies: nodes.iter().map(|n| n.copies).max().unwrap(),
            size: nodes.iter().map(|n| n.size).sum(),
            ..Node::atom()
        }
    }
    fn finish(mut self) -> Node {
        let last = self.concat();
        self.branches.push(last);
        let mut node = if self.branches.len() == 1 {
            last
        } else {
            Node {
                height: 1 + self.branches.iter().map(|n| n.height).max().unwrap(),
                copies: self.branches.iter().map(|n| n.copies).max().unwrap(),
                size: self.branches.iter().map(|n| n.size).sum::<u64>()
                    + self.branches.len() as u64
                    - 1,
                ..Node::atom()
            }
        };
        if self.capture {
            node.height += 1;
            node.size += 2;
            node.literal = false;
        }
        node
    }
    // Keep the last literal separate because a subsequent repeat applies only
    // to it; merge the preceding two before pushing the next atom, like Go.
    fn push(&mut self, node: Node) {
        let n = self.nodes.len();
        if n >= 2
            && self.nodes[n - 1].literal
            && self.nodes[n - 2].literal
            && self.nodes[n - 1].fold == self.nodes[n - 2].fold
        {
            let last = self.nodes.pop().unwrap();
            self.nodes.last_mut().unwrap().size += last.size;
        }
        self.nodes.push(node);
    }
}
struct Parser<'a> {
    pattern: &'a str,
    pos: usize,
    fold: bool,
    frames: Vec<Frame>,
}
impl<'a> Parser<'a> {
    fn remaining(&self) -> &'a str {
        &self.pattern[self.pos..]
    }
    fn rune(&mut self) -> Option<u32> {
        let c = self.remaining().chars().next()?;
        self.pos += c.len_utf8();
        Some(c as u32)
    }
    fn escape(&mut self) -> Result<u32, String> {
        let start = self.pos;
        self.pos += 1;
        let Some(c) = self.rune() else {
            return Err(error("trailing backslash at end of expression", ""));
        };
        if c < 128 && !(c as u8).is_ascii_alphanumeric() {
            return Ok(c);
        }
        if (b'0' as u32..=b'7' as u32).contains(&c) {
            let following = self
                .remaining()
                .as_bytes()
                .first()
                .is_some_and(|b| (b'0'..=b'7').contains(b));
            if c == b'0' as u32 || following {
                let mut n = c - b'0' as u32;
                for _ in 1..3 {
                    let Some(&b) = self.remaining().as_bytes().first() else {
                        break;
                    };
                    if !(b'0'..=b'7').contains(&b) {
                        break;
                    }
                    self.pos += 1;
                    n = n * 8 + u32::from(b - b'0');
                }
                return Ok(n);
            }
        }
        if c == b'x' as u32 && !self.remaining().is_empty() {
            let x = self.rune().unwrap();
            if x == b'{' as u32 {
                let mut value = 0u32;
                let mut count = 0;
                loop {
                    let Some(next) = self.rune() else {
                        break;
                    };
                    if next == b'}' as u32 {
                        if count > 0 {
                            return Ok(value);
                        }
                        break;
                    }
                    let Some(hex) = char::from_u32(next).and_then(|c| c.to_digit(16)) else {
                        break;
                    };
                    value = value.saturating_mul(16).saturating_add(hex);
                    if value > 0x10ffff {
                        break;
                    }
                    count += 1;
                }
            } else {
                let y = self.rune();
                if let (Some(x), Some(y)) = (
                    char::from_u32(x).and_then(|c| c.to_digit(16)),
                    y.and_then(char::from_u32).and_then(|c| c.to_digit(16)),
                ) {
                    return Ok(x * 16 + y);
                }
            }
        } else if let Some(value) = match c {
            97 => Some(7),
            102 => Some(12),
            110 => Some(10),
            114 => Some(13),
            116 => Some(9),
            118 => Some(11),
            _ => None,
        } {
            return Ok(value);
        }
        Err(error(
            "invalid escape sequence",
            &self.pattern[start..self.pos],
        ))
    }
    fn unicode_class(&mut self) -> Result<bool, String> {
        if !(self.remaining().starts_with("\\p") || self.remaining().starts_with("\\P")) {
            return Ok(false);
        }
        let start = self.pos;
        self.pos += 2;
        let name = if self.remaining().starts_with('{') {
            let Some(end) = self.remaining().find('}') else {
                return Err(error(
                    "invalid character class range",
                    &self.pattern[start..],
                ));
            };
            let name = &self.pattern[self.pos + 1..self.pos + end];
            self.pos += end + 1;
            name
        } else {
            let name_start = self.pos;
            self.rune();
            &self.pattern[name_start..self.pos]
        };
        let name = canonical(name.strip_prefix('^').unwrap_or(name));
        if NAMES.binary_search(&name.as_str()).is_err() {
            return Err(error(
                "invalid character class range",
                &self.pattern[start..self.pos],
            ));
        }
        Ok(true)
    }
    fn perl_class(&mut self) -> bool {
        if self
            .remaining()
            .as_bytes()
            .get(..2)
            .is_some_and(|s| s[0] == b'\\' && b"dDsSwW".contains(&s[1]))
        {
            self.pos += 2;
            true
        } else {
            false
        }
    }
    fn class_char(&mut self, start: usize) -> Result<u32, String> {
        if self.remaining().is_empty() {
            return Err(error("missing closing ]", &self.pattern[start..]));
        }
        if self.remaining().starts_with('\\') {
            self.escape()
        } else {
            Ok(self.rune().unwrap())
        }
    }
    fn class(&mut self) -> Result<(), String> {
        let start = self.pos;
        self.pos += 1;
        if self.remaining().starts_with('^') {
            self.pos += 1;
        }
        let mut first = true;
        while self.remaining().is_empty() || !self.remaining().starts_with(']') || first {
            first = false;
            if self.remaining().starts_with("[:") {
                if let Some(end) = self.remaining()[2..].find(":]") {
                    let length = end + 4;
                    let seq = &self.remaining()[..length];
                    self.pos += length;
                    let name = seq[2..seq.len() - 2]
                        .strip_prefix('^')
                        .unwrap_or(&seq[2..seq.len() - 2]);
                    if ![
                        "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower",
                        "print", "punct", "space", "upper", "word", "xdigit",
                    ]
                    .contains(&name)
                    {
                        return Err(error("invalid character class range", seq));
                    }
                    continue;
                }
            }
            if self.unicode_class()? || self.perl_class() {
                continue;
            }
            let range_start = self.pos;
            let low = self.class_char(start)?;
            if self.remaining().len() >= 2
                && self.remaining().starts_with('-')
                && !self.remaining()[1..].starts_with(']')
            {
                self.pos += 1;
                let high = self.class_char(start)?;
                if high < low {
                    return Err(error(
                        "invalid character class range",
                        &self.pattern[range_start..self.pos],
                    ));
                }
            }
        }
        self.pos += 1;
        Ok(())
    }
    fn group(&mut self) -> Result<(), String> {
        let start = self.pos;
        if !self.remaining().starts_with("(?") {
            self.pos += 1;
            self.frames.push(Frame::new(self.fold, true));
            return Ok(());
        }
        let rest = self.remaining();
        let named = if rest.len() > 4 && rest.starts_with("(?P<") {
            Some(4)
        } else if rest.len() > 3 && rest.starts_with("(?<") {
            Some(3)
        } else {
            None
        };
        if let Some(prefix) = named {
            let Some(end) = rest.find('>') else {
                return Err(error("invalid named capture", rest));
            };
            let name = &rest[prefix..end];
            self.pos += end + 1;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(error(
                    "invalid named capture",
                    &self.pattern[start..self.pos],
                ));
            }
            self.frames.push(Frame::new(self.fold, true));
            return Ok(());
        }
        self.pos += 2;
        let old = self.fold;
        let mut minus = false;
        let mut saw = false;
        while let Some(c) = self.rune() {
            match c {
                105 => {
                    self.fold = !minus;
                    saw = true;
                }
                109 | 115 | 85 => saw = true,
                45 if !minus => {
                    minus = true;
                    saw = false;
                }
                58 | 41 if !minus || saw => {
                    if c == 58 {
                        self.frames.push(Frame::new(old, false));
                    }
                    return Ok(());
                }
                _ => break,
            }
        }
        Err(error(
            "invalid or unsupported Perl syntax",
            &self.pattern[start..self.pos],
        ))
    }
}
fn integer(pattern: &str, pos: &mut usize) -> Option<i64> {
    let rest = &pattern.as_bytes()[*pos..];
    if !rest.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    if rest.len() > 1 && rest[0] == b'0' && rest[1].is_ascii_digit() {
        return None;
    }
    let mut n = 0i64;
    while *pos < pattern.len() && pattern.as_bytes()[*pos].is_ascii_digit() {
        let digit = i64::from(pattern.as_bytes()[*pos] - b'0');
        *pos += 1;
        if n >= 100000000 {
            n = -1;
        } else if n >= 0 {
            n = n * 10 + digit;
        }
    }
    Some(n)
}
fn counted(pattern: &str, pos: usize) -> Option<(u64, Option<u64>, usize)> {
    let mut i = pos + 1;
    let mut min = integer(pattern, &mut i)?;
    let mut max = min;
    if pattern.as_bytes().get(i) == Some(&b',') {
        i += 1;
        max = if pattern.as_bytes().get(i) == Some(&b'}') {
            -1
        } else {
            let n = integer(pattern, &mut i)?;
            if n < 0 {
                min = -1;
            }
            n
        };
    }
    if pattern.as_bytes().get(i) != Some(&b'}') {
        return None;
    }
    Some((
        if min < 0 { u64::MAX } else { min as u64 },
        if max < 0 { None } else { Some(max as u64) },
        i + 1,
    ))
}
pub(super) fn parse(pattern: &str) -> Result<(), String> {
    let mut p = Parser {
        pattern,
        pos: 0,
        fold: false,
        frames: vec![Frame::new(false, false)],
    };
    let mut last_repeat = None;
    while !p.remaining().is_empty() {
        let start = p.pos;
        let byte = p.remaining().as_bytes()[0];
        let mut repeat = None;
        match byte {
            b'(' => p.group()?,
            b')' => {
                p.pos += 1;
                if p.frames.len() == 1 {
                    return Err(error("unexpected )", pattern));
                }
                let frame = p.frames.pop().unwrap();
                p.fold = frame.flags;
                let node = frame.finish();
                if node.height > 1000 {
                    return Err(error("expression nests too deeply", pattern));
                }
                p.frames.last_mut().unwrap().push(node);
            }
            b'|' => {
                p.pos += 1;
                let frame = p.frames.last_mut().unwrap();
                let node = frame.concat();
                frame.branches.push(node);
            }
            b'*' | b'+' | b'?' | b'{' => {
                let count = if byte == b'{' {
                    counted(pattern, start)
                } else {
                    Some((0, None, start + 1))
                };
                if let Some((min, max, end)) = count {
                    if byte == b'{' && (min > 1000 || max.is_some_and(|m| m > 1000 || m < min)) {
                        return Err(error("invalid repeat count", &pattern[start..end]));
                    }
                    p.pos = end;
                    if p.remaining().starts_with('?') {
                        p.pos += 1;
                    }
                    if let Some(previous) = last_repeat {
                        return Err(error(
                            "invalid nested repetition operator",
                            &pattern[previous..p.pos],
                        ));
                    }
                    let frame = p.frames.last_mut().unwrap();
                    let Some(child) = frame.nodes.last_mut() else {
                        return Err(error(
                            "missing argument to repetition operator",
                            &pattern[start..p.pos],
                        ));
                    };
                    let n = match byte {
                        b'*' => Node {
                            height: child.height + 1,
                            size: child.size + 2,
                            ..*child
                        },
                        b'+' | b'?' => Node {
                            height: child.height + 1,
                            size: child.size + 1,
                            ..*child
                        },
                        _ => {
                            let copies = if max == Some(0) {
                                0
                            } else {
                                child.copies.saturating_mul(max.unwrap_or(min))
                            };
                            if (min >= 2 || max.is_some_and(|m| m >= 2)) && copies > 1000 {
                                return Err(error("invalid repeat count", &pattern[start..p.pos]));
                            }
                            let size = match max {
                                Some(m) => m.saturating_mul(child.size).saturating_add(m - min),
                                None if min == 0 => child.size + 2,
                                None => 1 + min.saturating_mul(child.size),
                            };
                            Node {
                                height: child.height + 1,
                                copies,
                                size: size.max(1),
                                ..*child
                            }
                        }
                    };
                    *child = Node {
                        literal: false,
                        ..n
                    };
                    if child.height > 1000 {
                        return Err(error("expression nests too deeply", pattern));
                    }
                    repeat = Some(start);
                } else {
                    p.pos += 1;
                    p.frames.last_mut().unwrap().push(Node::literal(p.fold));
                }
            }
            b'[' => {
                p.class()?;
                p.frames.last_mut().unwrap().push(Node::atom());
            }
            b'\\' => {
                if p.remaining().starts_with("\\Q") {
                    p.pos += 2;
                    let rest = p.remaining();
                    let end = rest.find("\\E").unwrap_or(rest.len());
                    for _ in rest[..end].chars() {
                        p.frames.last_mut().unwrap().push(Node::literal(p.fold));
                    }
                    p.pos += end;
                    if p.remaining().starts_with("\\E") {
                        p.pos += 2;
                    }
                } else if p
                    .remaining()
                    .as_bytes()
                    .get(1)
                    .is_some_and(|b| b"AbBz".contains(b))
                {
                    p.pos += 2;
                    p.frames.last_mut().unwrap().push(Node::atom());
                } else if p.unicode_class()? || p.perl_class() {
                    p.frames.last_mut().unwrap().push(Node::atom());
                } else {
                    p.escape()?;
                    p.frames.last_mut().unwrap().push(Node::literal(p.fold));
                }
            }
            b'^' | b'$' | b'.' => {
                p.pos += 1;
                p.frames.last_mut().unwrap().push(Node::atom());
            }
            _ => {
                p.rune();
                p.frames.last_mut().unwrap().push(Node::literal(p.fold));
            }
        }
        last_repeat = repeat;
    }
    if p.frames.len() != 1 {
        return Err(error("missing closing )", pattern));
    }
    let node = p.frames.pop().unwrap().finish();
    if node.height > 1000 {
        return Err(error("expression nests too deeply", pattern));
    }
    Ok(())
}
