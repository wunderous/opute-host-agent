// Copyright 2010 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// See licenses/Go-BSD.txt.
//! Byte-level Go encoding/json syntax diagnostics, following Go 1.25's scanner.
//! Kept separate from the ordered value parser so syntax is checked before types.
#[derive(Clone, Copy)]
enum State {
    Value,
    Array,
    Object,
    Key,
    String,
    Escape,
    Hex(u8),
    Neg,
    Zero,
    Digits,
    Dot,
    Fraction,
    Exponent,
    Sign,
    ExpDigits,
    Word(&'static [u8], usize),
    End,
    Top,
}
#[derive(Clone, Copy)]
enum Container {
    Key,
    Value,
    Array,
}
struct Scanner {
    state: State,
    stack: Vec<Container>,
    top: bool,
}
fn space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}
fn error(c: u8, context: &str) -> String {
    let quoted = match c {
        b'\'' => "'\\''".into(),
        b'"' => "'\"'".into(),
        _ => {
            let q = crate::goerr::quote(&(c as char).to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    };
    format!("invalid character {quoted} {context}")
}
impl Scanner {
    fn push(&mut self, c: u8, container: Container, state: State) -> Result<(), String> {
        self.stack.push(container);
        self.state = state;
        if self.stack.len() > 10_000 {
            Err(error(c, "exceeded max depth"))
        } else {
            Ok(())
        }
    }
    fn pop(&mut self) {
        self.stack.pop();
        self.top = self.stack.is_empty();
        self.state = if self.top { State::Top } else { State::End };
    }
    fn step(&mut self, c: u8) -> Result<(), String> {
        use State::*;
        match self.state {
            Value => {
                if space(c) {
                    return Ok(());
                }
                match c {
                    b'{' => self.push(c, Container::Key, Object)?,
                    b'[' => self.push(c, Container::Array, Array)?,
                    b'"' => self.state = String,
                    b'-' => self.state = Neg,
                    b'0' => self.state = Zero,
                    b'1'..=b'9' => self.state = Digits,
                    b't' => self.state = Word(b"true", 1),
                    b'f' => self.state = Word(b"false", 1),
                    b'n' => self.state = Word(b"null", 1),
                    _ => return Err(error(c, "looking for beginning of value")),
                }
            }
            Array => {
                if space(c) {
                    return Ok(());
                }
                if c == b']' {
                    self.state = End;
                    self.step(c)?;
                } else {
                    self.state = Value;
                    self.step(c)?;
                }
            }
            Object => {
                if space(c) {
                    return Ok(());
                }
                if c == b'}' {
                    *self.stack.last_mut().unwrap() = Container::Value;
                    self.state = End;
                    self.step(c)?;
                } else {
                    self.state = Key;
                    self.step(c)?;
                }
            }
            Key => {
                if space(c) {
                    return Ok(());
                }
                if c == b'"' {
                    self.state = String;
                } else {
                    return Err(error(c, "looking for beginning of object key string"));
                }
            }
            String => match c {
                b'"' => self.state = End,
                b'\\' => self.state = Escape,
                0..=31 => return Err(error(c, "in string literal")),
                _ => {}
            },
            Escape => match c {
                b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => self.state = String,
                b'u' => self.state = Hex(4),
                _ => return Err(error(c, "in string escape code")),
            },
            Hex(left) => {
                if !c.is_ascii_hexdigit() {
                    return Err(error(c, "in \\u hexadecimal character escape"));
                }
                self.state = if left == 1 { String } else { Hex(left - 1) };
            }
            Neg => match c {
                b'0' => self.state = Zero,
                b'1'..=b'9' => self.state = Digits,
                _ => return Err(error(c, "in numeric literal")),
            },
            Digits => {
                if !c.is_ascii_digit() {
                    self.state = Zero;
                    self.step(c)?;
                }
            }
            Zero => match c {
                b'.' => self.state = Dot,
                b'e' | b'E' => self.state = Exponent,
                _ => {
                    self.state = End;
                    self.step(c)?;
                }
            },
            Dot => {
                if !c.is_ascii_digit() {
                    return Err(error(c, "after decimal point in numeric literal"));
                }
                self.state = Fraction;
            }
            Fraction => {
                if matches!(c, b'e' | b'E') {
                    self.state = Exponent;
                } else if !c.is_ascii_digit() {
                    self.state = End;
                    self.step(c)?;
                }
            }
            Exponent => {
                self.state = Sign;
                if !matches!(c, b'+' | b'-') {
                    self.step(c)?;
                }
            }
            Sign => {
                if !c.is_ascii_digit() {
                    return Err(error(c, "in exponent of numeric literal"));
                }
                self.state = ExpDigits;
            }
            ExpDigits => {
                if !c.is_ascii_digit() {
                    self.state = End;
                    self.step(c)?;
                }
            }
            Word(word, index) => {
                if c != word[index] {
                    return Err(error(
                        c,
                        &format!(
                            "in literal {} (expecting '{}')",
                            std::str::from_utf8(word).unwrap(),
                            word[index] as char
                        ),
                    ));
                }
                self.state = if index + 1 == word.len() {
                    End
                } else {
                    Word(word, index + 1)
                };
            }
            End => {
                let Some(container) = self.stack.last().copied() else {
                    self.state = Top;
                    self.top = true;
                    return self.step(c);
                };
                if space(c) {
                    return Ok(());
                }
                match container {
                    Container::Key => {
                        if c == b':' {
                            *self.stack.last_mut().unwrap() = Container::Value;
                            self.state = Value;
                        } else {
                            return Err(error(c, "after object key"));
                        }
                    }
                    Container::Value => match c {
                        b',' => {
                            *self.stack.last_mut().unwrap() = Container::Key;
                            self.state = Key;
                        }
                        b'}' => self.pop(),
                        _ => return Err(error(c, "after object key:value pair")),
                    },
                    Container::Array => match c {
                        b',' => self.state = Value,
                        b']' => self.pop(),
                        _ => return Err(error(c, "after array element")),
                    },
                }
            }
            Top => {
                if !space(c) {
                    return Err(error(c, "after top-level value"));
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn syntax(input: &[u8]) -> Result<(), String> {
    let mut s = Scanner {
        state: State::Value,
        stack: Vec::new(),
        top: false,
    };
    for &c in input {
        s.step(c)?;
    }
    if s.top {
        return Ok(());
    }
    s.step(b' ')?;
    if s.top {
        Ok(())
    } else {
        Err("unexpected end of JSON input".into())
    }
}
