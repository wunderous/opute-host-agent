//! Modern-only outbound MCP edge. Provider generation ownership stays in the
//! parent module; this client never negotiates legacy sessions or executes plans.
use super::definition::{validate_descriptor, PluginDescriptor};
use crate::{goerr::quote, plan::runner::RunCtx};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;
use tokio::sync::RwLock;

const PROTOCOL: &str = "2026-07-28";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    token: String,
    name: String,
    version: String,
}
impl Client {
    pub fn new(
        endpoint: String,
        token: String,
        name: String,
        version: String,
    ) -> Result<Self, String> {
        let builder = reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_keepalive_interval(Duration::from_secs(15))
            .tcp_keepalive_retries(9);
        #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
        let builder = builder.tcp_user_timeout(None);
        let http = builder.build().map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            endpoint,
            token,
            name,
            version,
        })
    }
    pub async fn call(
        &self,
        ctx: &RunCtx,
        method: &str,
        name: &str,
        mut params: Map<String, Value>,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        let mut meta = json!({
            "io.modelcontextprotocol/protocolVersion":PROTOCOL,
            "io.modelcontextprotocol/clientInfo":{"name":if self.name.is_empty(){"opute-host-agent"}else{&self.name},"version":if self.version.trim().is_empty(){"1.0.0"}else{&self.version}},
            "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}}
        });
        if !delegation.trim().is_empty() {
            meta["io.opute/host-resource-delegation"] =
                Value::String(delegation.trim().to_string());
        }
        params.insert("_meta".into(), meta);
        let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        let mut encoded = String::new();
        crate::gojson::encode(&body, &mut encoded);
        let mut request = self
            .http
            .post(self.endpoint.trim())
            .header("Content-Type", "application/json")
            .header("User-Agent", "Go-http-client/1.1")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .header("Mcp-Method", method)
            .body(encoded);
        if !name.is_empty() {
            request = request.header("Mcp-Name", name);
        }
        if !self.token.trim().is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.token));
        }
        let response = under_context(ctx, async {
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            Ok((status, bytes))
        })
        .await
        .map_err(|e| {
            if matches!(e.as_str(), "context canceled" | "context deadline exceeded") {
                format!("Post {}: {e}", quote(self.endpoint.trim()))
            } else {
                e
            }
        })?;
        let (status, bytes) = response;
        if status >= 400 {
            return Err(format!(
                "MCP HTTP {status}: {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        let envelope: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("decode MCP response: {e}"))?;
        if let Some(error) = envelope.get("error").filter(|e| !e.is_null()) {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(format!("MCP error {code}: {message}"));
        }
        Ok(envelope
            .get("result")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    }
}

// Dropping the selected HTTP future cancels the request, rather than leaving
// an unobserved blocking worker that can outlive its task's reservation.
async fn under_context<T>(
    ctx: &RunCtx,
    future: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if let Some(error) = ctx.err() {
        return Err(error);
    }
    tokio::pin!(future);
    loop {
        tokio::select! {
            result=&mut future=>return result,
            _=tokio::time::sleep(Duration::from_millis(5))=>if let Some(error)=ctx.err(){return Err(error);},
        }
    }
}

pub struct Adapter {
    descriptor: PluginDescriptor,
    client: Client,
    tools: BTreeSet<String>,
    closed: RwLock<bool>,
}
impl Adapter {
    pub async fn connect(
        ctx: &RunCtx,
        descriptor: PluginDescriptor,
        token: String,
        name: &str,
        version: &str,
    ) -> Result<Self, String> {
        validate_descriptor(&descriptor)?;
        let name = if name.trim().is_empty() {
            "opute-host-agent"
        } else {
            name.trim()
        };
        let version = if version.trim().is_empty() {
            "dev"
        } else {
            version.trim()
        };
        let client = Client::new(
            descriptor.server.endpoint.clone(),
            token,
            name.into(),
            version.into(),
        )?;
        let discover = client
            .call(ctx, "server/discover", "", Map::new(), "")
            .await
            .map_err(|e| format!("discover provider MCP: {e}"))?;
        if !discover
            .get("supportedVersions")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(|v| v == PROTOCOL))
        {
            return Err(format!(
                "provider MCP negotiated unsupported protocol; require {PROTOCOL}"
            ));
        }
        let listed = client
            .call(ctx, "tools/list", "", Map::new(), "")
            .await
            .map_err(|e| format!("list provider tools: {e}"))?;
        let mut tools = BTreeSet::new();
        for item in listed
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.trim().is_empty() {
                return Err("provider returned an invalid tool definition".into());
            }
            if !tools.insert(name.to_string()) {
                return Err(format!("provider returned duplicate tool {}", quote(name)));
            }
        }
        if !tools.contains("opute.provider.get_install_manifest") {
            return Err("provider does not expose opute.provider.get_install_manifest".into());
        }
        Ok(Self {
            descriptor,
            client,
            tools,
            closed: RwLock::new(false),
        })
    }
    pub fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().cloned().collect()
    }
    pub async fn close(&self) {
        *self.closed.write().await = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_closes_a_request_waiting_for_provider_headers() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let provider = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            started_tx.send(()).unwrap();
            loop {
                match socket.read(&mut buffer) {
                    Ok(0) => {
                        closed_tx.send(true).unwrap();
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        closed_tx.send(false).unwrap();
                        break;
                    }
                }
            }
        });
        let client = Client::new(
            format!("http://{address}/mcp"),
            String::new(),
            String::new(),
            String::new(),
        )
        .unwrap();
        let (ctx, cancel) = RunCtx::background().with_cancel();
        let cancelling = std::thread::spawn(move || {
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            cancel.cancel();
        });
        let result = client
            .call(&ctx, "server/discover", "", Map::new(), "")
            .await;
        assert_eq!(
            result.unwrap_err(),
            format!(
                "Post {}: context canceled",
                quote(&format!("http://{address}/mcp"))
            )
        );
        // Let the async connection driver observe the dropped request future.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(closed_rx.recv_timeout(Duration::from_secs(3)).unwrap());
        cancelling.join().unwrap();
        provider.join().unwrap();
    }

    #[test]
    #[ignore = "requires the isolated provider adapter differential driver"]
    fn adapter_fixture() {
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_ADAPTER_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let ctx = RunCtx::background();
            let descriptor = serde_json::from_value(input["descriptor"].clone()).unwrap();
            match Adapter::connect(
                &ctx,
                descriptor,
                input["token"].as_str().unwrap_or_default().into(),
                input["name"].as_str().unwrap_or_default(),
                input["version"].as_str().unwrap_or_default(),
            )
            .await
            {
                Ok(adapter) => json!({"tools":adapter.tool_names()}),
                Err(e) => json!({"error":e}),
            }
        });
        std::fs::write(
            std::env::var("PARITY_PROVIDER_ADAPTER_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }
}
