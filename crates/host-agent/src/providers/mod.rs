//! Provider executables (decision D26, `port-provider-executables`).
//!
//! Each provider is a Service Provider in the D14 sense: its install manifest
//! is the Service Definition, its operations are implemented here, and the
//! Host Agent's provider binding is the Consumer, reached over MCP exactly as
//! it reaches Go's executables. This module is what every Go provider `main`
//! shares: go-sdk v1.7.0's stateless, JSON-response Streamable HTTP handler
//! (`mcpsdk`) served by `http.ListenAndServe(":"+port, handler)`.
//!
//! ```text
//!  main   OPUTE_PROVIDER_PORT (default per provider) -> log.Printf listening
//!         -> net.Listen(":"+port) (error: log.Fatal, exit 1)
//!  serve  every path -> go-sdk handler (OPTIONS * answered by net/http)
//! ```
//!
//! The server has no Logger, so go-sdk's own log lines are discarded, and no
//! signal handler: SIGTERM ends the process as it ends Go's.

pub mod cloudflare;
pub mod hostos;
mod support;

pub use support::{go_any, go_panic, Args, HostAgent};
pub(crate) use support::proxy_for;

use crate::gojson::Node;
use crate::http1::{Body, Request, Response};
use crate::mcpsdk::{Sdk, ToolCallOutcome};
use serde_json::{json, Map, Value as J};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;

/// One `tools/call` as a handler sees it.
pub struct Call<'a> {
    /// `params.arguments`, absent when the call has none.
    pub arguments: Option<&'a Node>,
    /// `params._meta`, absent when the call has none.
    pub meta: Option<&'a Node>,
}

impl Call<'_> {
    /// `hostagentclient.ForwardResourceDelegation`: the caller's resource
    /// delegation, forwarded on every callback the call makes.
    pub fn delegation(&self) -> String {
        self.meta
            .and_then(|m| m.as_object())
            .and_then(|entries| {
                entries
                    .iter()
                    .rev()
                    .find(|(k, _)| k == "io.opute/host-resource-delegation")
            })
            .and_then(|(_, v)| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    }
}

/// A tool handler.
pub type Handler = Box<dyn Fn(&Call) -> ToolCallOutcome + Send + Sync>;

/// One registered tool: its `tools/list` entry and its handler.
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: J,
    pub output_schema: Option<J>,
    pub handler: Handler,
}

impl Tool {
    /// The `mcp.Tool` as go-sdk publishes it.
    fn listing(&self) -> J {
        let mut tool = Map::new();
        tool.insert("name".into(), J::from(self.name.as_str()));
        tool.insert("description".into(), J::from(self.description.as_str()));
        tool.insert("inputSchema".into(), self.input_schema.clone());
        if let Some(output) = &self.output_schema {
            tool.insert("outputSchema".into(), output.clone());
        }
        J::Object(tool)
    }
}

/// A provider's `mcp.Server`: its implementation, capabilities and tools.
pub struct Server {
    pub name: &'static str,
    pub version: &'static str,
    pub capabilities: J,
    pub tools: Vec<Tool>,
    /// `mcphttp.WrapProviderHandler`'s server info, for a provider whose
    /// handler is wrapped.
    pub wrap: Option<J>,
}

impl Server {
    fn serve(&self, req: &Request, raw: &[u8]) -> Response {
        if let Some(info) = &self.wrap {
            if let Some(response) = support::wrap_provider_handler(info, req, raw) {
                return response;
            }
        }
        let mut listing: Vec<(&str, J)> = self
            .tools
            .iter()
            .map(|t| (t.name.as_str(), t.listing()))
            .collect();
        listing.sort_by(|a, b| a.0.cmp(b.0));
        let listing: Vec<J> = listing.into_iter().map(|(_, t)| t).collect();
        let call = |name: &str, params: Option<&Node>| match self
            .tools
            .iter()
            .find(|t| t.name == name)
        {
            None => ToolCallOutcome::Unknown,
            Some(tool) => (tool.handler)(&Call {
                arguments: params.and_then(|p| p.field("arguments")),
                meta: params.and_then(|p| p.field("_meta")),
            }),
        };
        let sdk = Sdk {
            implementation_name: self.name,
            version: self.version,
            capabilities: &self.capabilities,
            log: false,
            tools: &listing,
            call_tool: &call,
        };
        crate::mcpsdk::serve(&sdk, req, raw, true)
    }
}

/// `structured(value)`: the value as text content (Go's `json.Marshal`,
/// passed as `text`) and as structured content.
pub fn structured(text: String, value: J) -> ToolCallOutcome {
    ToolCallOutcome::Result(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
    }))
}

/// Go's `json.Marshal` of a value whose key order the caller has fixed:
/// `encoding/json` escapes `<`, `>`, `&`, U+2028 and U+2029.
pub fn go_text(text: String) -> String {
    text.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// A Go `log` standard-flags line (`2006/01/02 15:04:05 msg`), in UTC as
/// the agent's slog lines are.
pub fn log_printf(stderr: &mut dyn Write, msg: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (y, m, d) = crate::app::civil_from_days(now.div_euclid(86_400));
    let rem = now.rem_euclid(86_400);
    let _ = writeln!(
        stderr,
        "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02} {msg}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    );
    let _ = stderr.flush();
}

/// Go's `parsePort` and `lookupPort("tcp", service)` for `":"+port`: a
/// number in range, else a service name from `/etc/services` or Go's
/// built-in table, compared without case.
fn listen_port(port: &str) -> Result<u16, String> {
    let digits = port.strip_prefix(['+', '-']).unwrap_or(port);
    if !port.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        let negative = port.starts_with('-');
        return match digits.parse::<u32>() {
            Ok(n) if !negative && n <= 65_535 => Ok(n as u16),
            Ok(0) if negative => Ok(0),
            _ => Err(format!("listen tcp: address {port}: invalid port")),
        };
    }
    if port.is_empty() {
        return Ok(0);
    }
    let lower = port.to_ascii_lowercase();
    let builtin = [
        ("ftp", 21),
        ("ftps", 990),
        ("gopher", 70),
        ("http", 80),
        ("https", 443),
        ("imap2", 143),
        ("imap3", 220),
        ("imaps", 993),
        ("pop3", 110),
        ("pop3s", 995),
        ("smtp", 25),
        ("submissions", 465),
        ("ssh", 22),
        ("telnet", 23),
    ];
    let mut table: Vec<(String, u16)> = builtin.iter().map(|(n, p)| (n.to_string(), *p)).collect();
    // readServices: "name port/proto alias...", '#' starts a comment; a
    // later line overrides an earlier one.
    if let Ok(text) = std::fs::read_to_string("/etc/services") {
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            let fields: Vec<&str> = line.split_whitespace().collect();
            let Some((number, proto)) = fields.get(1).and_then(|f| f.split_once('/')) else {
                continue;
            };
            let Ok(number) = number.parse::<u16>() else {
                continue;
            };
            if proto != "tcp" {
                continue;
            }
            table.retain(|(n, _)| n != fields[0] && !fields[2..].contains(&n.as_str()));
            table.push((fields[0].to_string(), number));
            for alias in &fields[2..] {
                table.push((alias.to_string(), number));
            }
        }
    }
    table
        .iter()
        .rev()
        .find(|(name, _)| *name == lower)
        .map(|(_, p)| *p)
        .ok_or_else(|| format!("listen tcp: lookup tcp/{port}: unknown port"))
}

/// The provider `main`: read the port, log, listen on every interface and
/// serve until the process is killed. A listen failure is `log.Fatal`.
pub fn run(server: Server, default_port: &str, listening: impl Fn(&str) -> String) -> ! {
    let mut port = std::env::var("OPUTE_PROVIDER_PORT").unwrap_or_default();
    if port.is_empty() {
        port = default_port.to_string();
    }
    let mut stderr = std::io::stderr();
    log_printf(&mut stderr, &listening(&port));
    let listener = listen_port(&port).and_then(|number| {
        let addr = format!(":{number}");
        let addrs: Vec<SocketAddr> = vec![SocketAddr::from(([0, 0, 0, 0], number))];
        crate::app::go_listen(&addrs, &addr).map_err(|e| e.0)
    });
    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            log_printf(&mut stderr, &e);
            std::process::exit(1);
        }
    };
    let server = Arc::new(server);
    // Go accepts JSON nested 10000 levels deep; the decoder recurses once per
    // level, so the runtime gets a large stack.
    let worker = std::thread::Builder::new()
        .name("http".into())
        .stack_size(256 << 20)
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .expect("start runtime");
            rt.block_on(accept(listener, server));
        })
        .expect("start runtime");
    let _ = worker.join();
    std::process::exit(1)
}

async fn accept(listener: std::net::TcpListener, server: Arc<Server>) {
    let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let server = server.clone();
                tokio::task::spawn_local(crate::http1::serve_conn(stream, move |req, body| {
                    handle(server.clone(), req, body)
                }));
            }
        })
        .await
}

async fn handle(server: Arc<Server>, req: Request, mut body: Body) -> (Response, Body) {
    // net/http answers `OPTIONS *` itself, before any handler.
    if req.request_uri == "*" && req.method == "OPTIONS" {
        let mut r = Response::new(200);
        r.headers.set("Content-Length", "0");
        return (r, body);
    }
    let raw = if req.method == "POST" {
        match body.read_all(None).await {
            Ok(b) => b,
            Err(e) => return (Response::error(&e.to_string(), 400), body),
        }
    } else {
        Vec::new()
    };
    // Tool handlers read the host; keep them off the connection's thread.
    let response = tokio::task::spawn_blocking(move || server.serve(&req, &raw))
        .await
        .unwrap_or_else(|_| Response::error("internal error", 500));
    (response, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_follow_go() {
        assert_eq!(listen_port("4321"), Ok(4321));
        assert_eq!(listen_port("+80"), Ok(80));
        assert_eq!(listen_port("http"), Ok(80));
        assert_eq!(listen_port("HTTP"), Ok(80));
        assert_eq!(
            listen_port("70000").unwrap_err(),
            "listen tcp: address 70000: invalid port"
        );
        assert_eq!(
            listen_port("-1").unwrap_err(),
            "listen tcp: address -1: invalid port"
        );
        assert_eq!(
            listen_port("abc").unwrap_err(),
            "listen tcp: lookup tcp/abc: unknown port"
        );
        assert_eq!(
            listen_port("1 ").unwrap_err(),
            "listen tcp: lookup tcp/1 : unknown port"
        );
    }
}
