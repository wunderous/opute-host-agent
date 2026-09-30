//! A reimplementation of the subset of Go's `flag` package the CLI uses.
//!
//! The Go Host Agent's command line is part of its operator contract: flag
//! syntax, `-h` output, default-value rendering and error text are observed by
//! installers, the npm launcher and humans. A general-purpose Rust argument
//! parser would change all of these, so this module follows Go's
//! `flag.FlagSet` with `ContinueOnError` semantics exactly:
//!
//! * `-name`, `--name`, `-name=value`, `-name value` (non-bool only);
//! * parsing stops at the first non-flag argument or after `--`;
//! * `-h`/`-help` print usage and return [`ParseError::Help`];
//! * other errors print the message, then usage, and return the message
//!   (Go's `failf`), so the caller prints it a second time.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::goerr::quote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    String,
    Bool,
    /// A repeatable `flag.Value` (Go prints its type as "value").
    Multi,
}

#[derive(Debug, Clone)]
struct Flag {
    kind: Kind,
    usage: &'static str,
    default: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// `flag.ErrHelp`; Go's main prints "flag: help requested".
    Help,
    /// Any other parse failure; the message was already written once.
    Failed(String),
}

impl ParseError {
    pub fn message(&self) -> String {
        match self {
            ParseError::Help => "flag: help requested".to_string(),
            ParseError::Failed(m) => m.clone(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Values {
    strings: BTreeMap<String, String>,
    bools: BTreeMap<String, bool>,
    multi: BTreeMap<String, Vec<String>>,
    /// Positional arguments left after flag parsing (Go's `fs.Args()`).
    pub rest: Vec<String>,
}

impl Values {
    pub fn string(&self, name: &str) -> String {
        self.strings.get(name).cloned().unwrap_or_default()
    }
    pub fn bool(&self, name: &str) -> bool {
        self.bools.get(name).copied().unwrap_or(false)
    }
    pub fn multi(&self, name: &str) -> Vec<String> {
        self.multi.get(name).cloned().unwrap_or_default()
    }
}

pub struct FlagSet {
    name: String,
    flags: BTreeMap<&'static str, Flag>,
}

impl FlagSet {
    pub fn new(name: impl Into<String>) -> Self {
        FlagSet {
            name: name.into(),
            flags: BTreeMap::new(),
        }
    }

    pub fn string(
        mut self,
        name: &'static str,
        default: &'static str,
        usage: &'static str,
    ) -> Self {
        self.flags.insert(
            name,
            Flag {
                kind: Kind::String,
                usage,
                default,
            },
        );
        self
    }

    pub fn bool(mut self, name: &'static str, default: bool, usage: &'static str) -> Self {
        let default = if default { "true" } else { "false" };
        self.flags.insert(
            name,
            Flag {
                kind: Kind::Bool,
                usage,
                default,
            },
        );
        self
    }

    pub fn multi(mut self, name: &'static str, usage: &'static str) -> Self {
        self.flags.insert(
            name,
            Flag {
                kind: Kind::Multi,
                usage,
                default: "",
            },
        );
        self
    }

    /// Go's `defaultUsage`: "Usage of NAME:" followed by `PrintDefaults`.
    pub fn usage(&self) -> String {
        let mut out = if self.name.is_empty() {
            "Usage:\n".to_string()
        } else {
            format!("Usage of {}:\n", self.name)
        };
        for (name, flag) in &self.flags {
            let mut line = format!("  -{name}");
            let type_name = match flag.kind {
                Kind::String => "string",
                Kind::Bool => "",
                Kind::Multi => "value",
            };
            if !type_name.is_empty() {
                line.push(' ');
                line.push_str(type_name);
            }
            if line.len() <= 4 {
                line.push('\t');
            } else {
                line.push_str("\n    \t");
            }
            line.push_str(&flag.usage.replace('\n', "\n    \t"));
            let is_zero = match flag.kind {
                Kind::String | Kind::Multi => flag.default.is_empty(),
                Kind::Bool => flag.default == "false",
            };
            if !is_zero {
                if flag.kind == Kind::String {
                    let _ = write!(line, " (default {})", quote(flag.default));
                } else {
                    let _ = write!(line, " (default {})", flag.default);
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Parse `args`. Diagnostics and usage go to `stderr`, as Go's
    /// `fs.SetOutput(stderr)` does.
    pub fn parse(&self, args: &[String], stderr: &mut String) -> Result<Values, ParseError> {
        let mut values = Values::default();
        for (name, flag) in &self.flags {
            match flag.kind {
                Kind::String => {
                    values
                        .strings
                        .insert((*name).to_string(), flag.default.to_string());
                }
                Kind::Bool => {
                    values
                        .bools
                        .insert((*name).to_string(), flag.default == "true");
                }
                Kind::Multi => {}
            }
        }
        let mut i = 0;
        while i < args.len() {
            let s = &args[i];
            if s.len() < 2 || !s.starts_with('-') {
                break;
            }
            let mut minuses = 1;
            if s.as_bytes()[1] == b'-' {
                minuses = 2;
                if s.len() == 2 {
                    i += 1;
                    break;
                }
            }
            let mut name = &s[minuses..];
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return Err(self.fail(format!("bad flag syntax: {s}"), stderr));
            }
            i += 1;
            let mut value: Option<&str> = None;
            // Go searches from index 1 ("equals cannot be first"), by byte.
            if let Some(eq) = name.as_bytes()[1..].iter().position(|b| *b == b'=') {
                let eq = eq + 1;
                value = Some(&name[eq + 1..]);
                name = &name[..eq];
            }
            let Some(flag) = self.flags.get(name) else {
                if name == "help" || name == "h" {
                    stderr.push_str(&self.usage());
                    return Err(ParseError::Help);
                }
                return Err(self.fail(format!("flag provided but not defined: -{name}"), stderr));
            };
            match flag.kind {
                Kind::Bool => {
                    let v = value.unwrap_or("true");
                    match parse_bool(v) {
                        Some(b) => {
                            values.bools.insert(name.to_string(), b);
                        }
                        None => {
                            return Err(self.fail(
                                format!(
                                    "invalid boolean value {} for -{name}: parse error",
                                    quote(v)
                                ),
                                stderr,
                            ))
                        }
                    }
                }
                Kind::String | Kind::Multi => {
                    let v = match value {
                        Some(v) => v.to_string(),
                        None if i < args.len() => {
                            i += 1;
                            args[i - 1].clone()
                        }
                        None => {
                            return Err(
                                self.fail(format!("flag needs an argument: -{name}"), stderr)
                            )
                        }
                    };
                    if flag.kind == Kind::String {
                        values.strings.insert(name.to_string(), v);
                    } else {
                        values.multi.entry(name.to_string()).or_default().push(v);
                    }
                }
            }
        }
        values.rest = args[i..].to_vec();
        Ok(values)
    }

    fn fail(&self, message: String, stderr: &mut String) -> ParseError {
        stderr.push_str(&message);
        stderr.push('\n');
        stderr.push_str(&self.usage());
        ParseError::Failed(message)
    }
}

/// Go `strconv.ParseBool`.
fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve() -> FlagSet {
        FlagSet::new("serve")
            .string("mode", "", "agent profile: standalone or platform")
            .string("transport", "", "MCP transport: http")
            .string("env-file", "", "load KEY=VALUE settings from a file")
            .bool(
                "check",
                false,
                "validate configuration and state access, then exit",
            )
            .multi("env", "set a KEY=VALUE environment override; repeatable")
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn usage_matches_go_print_defaults() {
        let expected = "Usage of serve:\n  -check\n    \tvalidate configuration and state access, then exit\n  -env value\n    \tset a KEY=VALUE environment override; repeatable\n  -env-file string\n    \tload KEY=VALUE settings from a file\n  -mode string\n    \tagent profile: standalone or platform\n  -transport string\n    \tMCP transport: http\n";
        assert_eq!(serve().usage(), expected);
    }

    #[test]
    fn defaults_are_rendered_like_go() {
        let fs = FlagSet::new("x")
            .string("scope", "user", "scope")
            .bool("wait", true, "wait")
            .bool("resume", false, "resume");
        let usage = fs.usage();
        assert!(usage.contains("  -scope string\n    \tscope (default \"user\")\n"));
        assert!(usage.contains("  -wait\n    \twait (default true)\n"));
        assert!(usage.contains("  -resume\n    \tresume\n"));
    }

    #[test]
    fn parses_forms_and_stops_at_positional() {
        let mut err = String::new();
        let v = serve()
            .parse(
                &args(&[
                    "--mode=standalone",
                    "-transport",
                    "http",
                    "--check",
                    "--env",
                    "A=1",
                    "-env=B=2",
                    "pos",
                    "--mode=x",
                ]),
                &mut err,
            )
            .unwrap();
        assert_eq!(v.string("mode"), "standalone");
        assert_eq!(v.string("transport"), "http");
        assert!(v.bool("check"));
        assert_eq!(v.multi("env"), vec!["A=1", "B=2"]);
        assert_eq!(v.rest, args(&["pos", "--mode=x"]));
    }

    #[test]
    fn double_dash_terminates() {
        let mut err = String::new();
        let v = serve().parse(&args(&["--", "--check"]), &mut err).unwrap();
        assert!(!v.bool("check"));
        assert_eq!(v.rest, args(&["--check"]));
    }

    #[test]
    fn errors_print_message_then_usage() {
        let mut err = String::new();
        let e = serve().parse(&args(&["--bogus"]), &mut err).unwrap_err();
        assert_eq!(
            e,
            ParseError::Failed("flag provided but not defined: -bogus".into())
        );
        assert!(err.starts_with("flag provided but not defined: -bogus\nUsage of serve:\n"));
        for (input, message) in [
            (vec!["---x"], "bad flag syntax: ---x"),
            (vec!["-=x"], "bad flag syntax: -=x"),
            (vec!["--mode"], "flag needs an argument: -mode"),
            (
                vec!["--check=maybe"],
                "invalid boolean value \"maybe\" for -check: parse error",
            ),
        ] {
            let mut err = String::new();
            let e = serve().parse(&args(&input), &mut err).unwrap_err();
            assert_eq!(e.message(), message);
        }
    }

    #[test]
    fn help_returns_err_help_after_usage() {
        for h in ["-h", "--help", "-help"] {
            let mut err = String::new();
            let e = serve().parse(&args(&[h]), &mut err).unwrap_err();
            assert_eq!(e, ParseError::Help);
            assert_eq!(err, serve().usage());
        }
    }

    #[test]
    fn bool_does_not_consume_next_arg() {
        let mut err = String::new();
        let v = serve()
            .parse(&args(&["--check", "false"]), &mut err)
            .unwrap();
        assert!(v.bool("check"));
        assert_eq!(v.rest, args(&["false"]));
    }
}
