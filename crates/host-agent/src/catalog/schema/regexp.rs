// Copyright 2009 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause; see licenses/Go-BSD.txt.
//! Native Go 1.25 regexp syntax admission. Pattern execution is not part of
//! catalog schema compilation. This private parser retains the owning lazy
//! resource caches; production registry admission remains unwired.
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
use super::regexp_tree::{self as tree, Arena, Kind};
struct Parser<'a> {
    pattern: &'a str,
    pos: usize,
    flags: u8,
    stack: Vec<usize>,
    arena: Arena,
}
impl<'a> Parser<'a> {
    fn concat(&mut self) -> Result<usize, String> {
        self.arena.merge_literals(&mut self.stack);
        let index = self
            .stack
            .iter()
            .rposition(|&id| self.arena.pseudo(id))
            .map_or(0, |i| i + 1);
        let children = self.stack.split_off(index);
        self.arena.context = self.stack.clone();
        let node = self.arena.collapse(Kind::Concat, children);
        let cost = self.arena.nodes[node].runes.len();
        self.push(node, cost)?;
        Ok(node)
    }
    fn alternate(&mut self) -> Result<usize, String> {
        let index = self
            .stack
            .iter()
            .rposition(|&id| self.arena.pseudo(id))
            .map_or(0, |i| i + 1);
        let children = self.stack.split_off(index);
        self.arena.clean_last(&children);
        self.arena.context = self.stack.clone();
        let node = self.arena.collapse(Kind::Alt, children);
        let cost = self.arena.nodes[node].runes.len();
        self.push(node, cost)?;
        Ok(node)
    }
    fn left_paren(&mut self, flags: u8, capture: bool) -> Result<(), String> {
        let node = self.arena.node(
            Kind::LeftParen,
            vec![],
            vec![],
            flags,
            u64::from(capture),
            None,
        );
        self.push(node, 0)
    }
    fn right_paren(&mut self) -> Result<(), String> {
        self.concat()?;
        if self.arena.swap_bar(&mut self.stack) {
            self.stack.pop();
        }
        self.alternate()?;
        if self.stack.len() < 2 {
            return Err(error("unexpected )", self.pattern));
        }
        let body = self.stack.pop().unwrap();
        let marker = self.stack.pop().unwrap();
        if self.arena.nodes[marker].kind != Kind::LeftParen {
            return Err(error("unexpected )", self.pattern));
        }
        self.flags = self.arena.nodes[marker].flags;
        let node = if self.arena.nodes[marker].min == 0 {
            body
        } else {
            self.arena.capture(marker, body);
            marker
        };
        let cost = self.arena.nodes[node].runes.len();
        self.push(node, cost)
    }
    fn folded(&self) -> bool {
        self.flags & 1 != 0
    }
    fn check(&mut self, node: usize) -> Result<(), String> {
        self.arena
            .limits(node, &self.stack)
            .map_err(|code| error(code, self.pattern))
    }
    fn push(&mut self, node: usize, cost: usize) -> Result<(), String> {
        self.arena.num_runes += cost;
        let class = self.arena.nodes[node].kind == Kind::Class;
        self.arena.normalize_push(node);
        if class && self.arena.nodes[node].kind == Kind::Literal {
            let n = &self.arena.nodes[node];
            if self
                .arena
                .maybe_concat(&mut self.stack, Some((n.runes[0], n.flags)))
            {
                return Ok(());
            }
        } else {
            self.arena.merge_literals(&mut self.stack);
        }
        self.stack.push(node);
        self.check(node)
    }
    fn literal(&mut self, r: u32) -> Result<(), String> {
        let folded = self.folded();
        let r = if folded { tree::min_fold(r) } else { r };
        let node = self.arena.literal(vec![r], folded);
        self.push(node, 1)
    }
    fn class_node(&mut self, ranges: Vec<u32>) -> Result<(), String> {
        let r = tree::clean(&ranges);
        let cost = r.len();
        let node = self.arena.class(r);
        self.push(node, cost)
    }
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
    fn unicode_class(&mut self) -> Result<Option<Vec<u32>>, String> {
        if !(self.remaining().starts_with("\\p") || self.remaining().starts_with("\\P")) {
            return Ok(None);
        }
        let start = self.pos;
        let mut negative = self.remaining().starts_with("\\P");
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
        let name = if let Some(name) = name.strip_prefix('^') {
            negative = !negative;
            name
        } else {
            name
        };
        let name = canonical(name);
        if NAMES.binary_search(&name.as_str()).is_err() {
            return Err(error(
                "invalid character class range",
                &self.pattern[start..self.pos],
            ));
        }
        let ranges = tree::table(&name, self.folded(), false).unwrap();
        Ok(Some(if negative {
            tree::negate(&ranges)
        } else {
            ranges
        }))
    }
    fn perl_class(&mut self) -> Option<Vec<u32>> {
        if self
            .remaining()
            .as_bytes()
            .get(..2)
            .is_some_and(|s| s[0] == b'\\' && b"dDsSwW".contains(&s[1]))
        {
            let b = self.remaining().as_bytes()[1];
            self.pos += 2;
            let name = match b.to_ascii_lowercase() {
                b'd' => "digit",
                b's' => "space",
                _ => "word",
            };
            let ranges = if b.eq_ignore_ascii_case(&b's') {
                vec![9, 10, 12, 13, 32, 32]
            } else {
                tree::table(name, self.folded(), true).unwrap()
            };
            Some(if b.is_ascii_uppercase() {
                tree::negate(&ranges)
            } else {
                ranges
            })
        } else {
            None
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
    fn class(&mut self) -> Result<Vec<u32>, String> {
        let start = self.pos;
        self.pos += 1;
        let negative = self.remaining().starts_with('^');
        if negative {
            self.pos += 1;
        }
        let mut ranges = Vec::new();
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
                    let mut class = tree::table(name, self.folded(), true).unwrap();
                    if seq[2..].starts_with('^') {
                        class = tree::negate(&class);
                    }
                    for p in class.chunks_exact(2) {
                        tree::append(&mut ranges, p[0], p[1]);
                    }
                    continue;
                }
            }
            if let Some(class) = self.unicode_class()?.or_else(|| self.perl_class()) {
                for p in class.chunks_exact(2) {
                    tree::append(&mut ranges, p[0], p[1]);
                }
                continue;
            }
            let range_start = self.pos;
            let low = self.class_char(start)?;
            let mut high = low;
            if self.remaining().len() >= 2
                && self.remaining().starts_with('-')
                && !self.remaining()[1..].starts_with(']')
            {
                self.pos += 1;
                high = self.class_char(start)?;
                if high < low {
                    return Err(error(
                        "invalid character class range",
                        &self.pattern[range_start..self.pos],
                    ));
                }
            }
            if self.folded() {
                tree::append_folded(&mut ranges, low, high);
            } else {
                tree::append(&mut ranges, low, high);
            }
        }
        self.pos += 1;
        let ranges = tree::clean(&ranges);
        Ok(if negative {
            tree::negate(&ranges)
        } else {
            ranges
        })
    }
    fn group(&mut self) -> Result<(), String> {
        let start = self.pos;
        if !self.remaining().starts_with("(?") {
            self.pos += 1;
            self.left_paren(self.flags, true)?;
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
            self.left_paren(self.flags, true)?;
            return Ok(());
        }
        self.pos += 2;
        let old = self.flags;
        let mut minus = false;
        let mut saw = false;
        while let Some(c) = self.rune() {
            match c {
                105 => {
                    if minus {
                        self.flags &= !1;
                    } else {
                        self.flags |= 1;
                    }
                    saw = true;
                }
                109 | 115 | 85 => {
                    let bit = match c {
                        109 => 2,
                        115 => 4,
                        _ => 8,
                    };
                    if minus {
                        self.flags &= !bit;
                    } else {
                        self.flags |= bit;
                    }
                    saw = true;
                }
                45 if !minus => {
                    minus = true;
                    saw = false;
                }
                58 | 41 if !minus || saw => {
                    if c == 58 {
                        self.left_paren(old, false)?;
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
        flags: 0,
        stack: vec![],
        arena: Arena::default(),
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
                p.right_paren()?;
            }
            b'|' => {
                p.pos += 1;
                p.concat()?;
                if !p.arena.swap_bar(&mut p.stack) {
                    let node = p
                        .arena
                        .node(Kind::VerticalBar, vec![], vec![], p.flags, 0, None);
                    p.push(node, 0)?;
                }
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
                    let mut nongreedy = p.flags & 8;
                    if p.remaining().starts_with('?') {
                        p.pos += 1;
                        nongreedy ^= 8;
                    }
                    if let Some(previous) = last_repeat {
                        return Err(error(
                            "invalid nested repetition operator",
                            &pattern[previous..p.pos],
                        ));
                    }
                    let Some(child) = p.stack.last().copied().filter(|&id| !p.arena.pseudo(id))
                    else {
                        return Err(error(
                            "missing argument to repetition operator",
                            &pattern[start..p.pos],
                        ));
                    };
                    let kind = match byte {
                        b'*' => Kind::Star,
                        b'+' => Kind::Plus,
                        b'?' => Kind::Quest,
                        _ => Kind::Repeat,
                    };
                    let node = p.arena.node(kind, vec![], vec![child], nongreedy, min, max);
                    *p.stack.last_mut().unwrap() = node;
                    p.check(node)?;
                    if kind == Kind::Repeat
                        && (min >= 2 || max.is_some_and(|m| m >= 2))
                        && p.arena.nodes[node].copies > 1000
                    {
                        return Err(error("invalid repeat count", &pattern[start..p.pos]));
                    }
                    repeat = Some(start);
                } else {
                    p.pos += 1;
                    p.literal(b'{' as u32)?;
                }
            }
            b'[' => {
                let ranges = p.class()?;
                p.class_node(ranges)?;
            }
            b'\\' => {
                if p.remaining().starts_with("\\Q") {
                    p.pos += 2;
                    let rest = p.remaining();
                    let end = rest.find("\\E").unwrap_or(rest.len());
                    for c in rest[..end].chars() {
                        p.literal(c as u32)?;
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
                    let b = p.remaining().as_bytes()[1];
                    p.pos += 2;
                    let node = p.arena.node(Kind::Atom(b), vec![], vec![], 0, 0, None);
                    p.push(node, 0)?;
                } else {
                    let dummy = p.arena.node(Kind::Class, vec![], vec![], p.flags, 0, None);
                    if let Some(ranges) = p.unicode_class()?.or_else(|| p.perl_class()) {
                        let cost = ranges.len();
                        p.arena.nodes[dummy].runes = ranges;
                        p.push(dummy, cost)?;
                    } else {
                        p.arena.reuse(dummy);
                        let r = p.escape()?;
                        p.literal(r)?;
                    }
                }
            }
            b'.' => {
                p.pos += 1;
                let kind = if p.flags & 4 != 0 {
                    Kind::Any
                } else {
                    Kind::AnyNotNL
                };
                let node = p.arena.node(kind, vec![], vec![], 0, 0, None);
                p.push(node, 0)?;
            }
            b'^' | b'$' => {
                p.pos += 1;
                let node = p
                    .arena
                    .node(Kind::Atom(byte), vec![], vec![], p.flags & 2, 0, None);
                p.push(node, 0)?;
            }
            _ => {
                let r = p.rune().unwrap();
                p.literal(r)?;
            }
        }
        last_repeat = repeat;
    }
    p.concat()?;
    if p.arena.swap_bar(&mut p.stack) {
        p.stack.pop();
    }
    p.alternate()?;
    if p.stack.len() != 1 {
        return Err(error("missing closing )", pattern));
    }
    Ok(())
}
