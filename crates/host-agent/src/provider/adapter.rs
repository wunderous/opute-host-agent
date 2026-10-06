//! Modern-only outbound MCP edge. Provider generation ownership stays in the
//! parent module; this client never negotiates legacy sessions or executes plans.
use super::definition::{
    decode_manifest, validate_descriptor, validate_install_manifest, InstallManifest,
    PluginDescriptor,
};
use crate::{goerr::quote, plan::runner::RunCtx};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::future::Future;
use std::time::Duration;
use tokio::sync::RwLock;

mod envelope;

const PROTOCOL: &str = "2026-07-28";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    token: String,
    name: String,
    version: String,
    #[cfg(test)]
    progress: Option<(String, String)>,
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
            #[cfg(test)]
            progress: None,
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
        let mut response = under_context(ctx, async {
            request.send().await.map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| {
            if matches!(e.as_str(), "context canceled" | "context deadline exceeded") {
                format!("Post {}: {e}", quote(self.endpoint.trim()))
            } else {
                e
            }
        })?;
        let status = response.status().as_u16();
        // Go http.Client.Do has returned at this boundary. ReadAll errors are
        // returned directly, rather than being wrapped in a Post url.Error.
        let bytes = under_context(ctx, async {
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                bytes.extend_from_slice(&chunk);
                #[cfg(test)]
                self.record_progress("body");
            }
            #[cfg(test)]
            self.record_progress("body-end");
            Ok(bytes)
        })
        .await?;
        if status >= 400 {
            return Err(format!(
                "MCP HTTP {status}: {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        envelope::decode(&bytes)
    }
    #[cfg(test)]
    fn record_progress(&self, stage: &str) {
        if let Some((wanted, path)) = &self.progress {
            if wanted == stage {
                std::fs::write(path, stage).expect("fixture progress marker");
            }
        }
    }

    // The accepted result crosses the same generic-map and SDK typed boundary
    // as the pinned Go client, after child-task polling.
    pub async fn call_tool(
        &self,
        ctx: &RunCtx,
        name: &str,
        arguments: Option<Map<String, Value>>,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        let params = json!({"name":name,"arguments":arguments})
            .as_object()
            .unwrap()
            .clone();
        let result = self
            .call(ctx, "tools/call", name, params, delegation)
            .await?;
        let task = result
            .get("taskId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let result = if result.get("resultType") == Some(&Value::String("task".into()))
            && !task.is_empty()
        {
            let interval = result
                .get("pollIntervalMs")
                .and_then(Value::as_f64)
                .filter(|v| *v >= 1.0 && *v <= 60_000.0)
                .map(|v| Duration::from_millis(v as u64))
                .unwrap_or(Duration::from_millis(500));
            self.wait_task(ctx, task, interval, delegation).await?
        } else {
            result
        };
        super::result::project(&result)
    }
    async fn wait_task(
        &self,
        ctx: &RunCtx,
        task: &str,
        interval: Duration,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        loop {
            let params = json!({"taskId":task}).as_object().unwrap().clone();
            let status = match self.call(ctx, "tasks/get", task, params, delegation).await {
                Ok(status) => status,
                Err(error) => {
                    return Err(self
                        .cancel_after_wait_failure(
                            task,
                            format!("get host task {task}: {error}"),
                            delegation,
                        )
                        .await)
                }
            };
            let status_name = status
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match status_name.trim().to_ascii_lowercase().as_str() {
                "completed" => {
                    return status
                        .get("result")
                        .and_then(Value::as_object)
                        .cloned()
                        .ok_or_else(|| format!("host task {task} completed without a tool result"))
                }
                "input_required" => {
                    return Err(self
                        .cancel_after_wait_failure(
                            task,
                            format!("host task {task} {status_name}: {}", task_message(&status)),
                            delegation,
                        )
                        .await)
                }
                "failed" | "cancelled" => {
                    return Err(format!(
                        "host task {task} {status_name}: {}",
                        task_message(&status)
                    ))
                }
                _ => {}
            }
            if let Err(error) = under_context(ctx, async {
                tokio::time::sleep(interval).await;
                Ok(())
            })
            .await
            {
                return Err(self
                    .cancel_after_wait_failure(task, error, delegation)
                    .await);
            }
        }
    }
    async fn cancel_after_wait_failure(
        &self,
        task: &str,
        cause: String,
        delegation: &str,
    ) -> String {
        let (ctx, _) = RunCtx::background().with_timeout(Duration::from_secs(2));
        let params = json!({"taskId":task}).as_object().unwrap().clone();
        let error = self
            .call(&ctx, "tasks/cancel", task, params, delegation)
            .await
            .err();
        match error {
            Some(error) => {
                format!("wait for host task {task}: {cause}; cancellation request failed: {error}")
            }
            None => format!("wait for host task {task}: {cause}"),
        }
    }
}

fn task_message(status: &Map<String, Value>) -> String {
    if let Some(message) = status
        .get("statusMessage")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return message.trim().into();
    }
    if let Some(message) = status
        .get("error")
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return message.trim().into();
    }
    "no diagnostic".into()
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
    pub async fn call(
        &self,
        ctx: &RunCtx,
        operation: &str,
        arguments: Option<Map<String, Value>>,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        let closed = self.closed.read().await;
        if *closed {
            return Err("provider adapter is closed".into());
        }
        if !self.tools.contains(operation) {
            return Err(format!(
                "provider operation {} is not registered",
                quote(operation)
            ));
        }
        self.client
            .call_tool(ctx, operation, arguments, delegation)
            .await
    }
    pub async fn call_synchronous_only(
        &self,
        ctx: &RunCtx,
        operation: &str,
        arguments: Option<Map<String, Value>>,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        let result = self.call(ctx, operation, arguments, delegation).await?;
        if let Some(content) = result.get("structuredContent").and_then(Value::as_object) {
            if content.get("resultType") == Some(&Value::String("task".into()))
                || content.get("taskId").is_some_and(|v| !v.is_null())
            {
                return Err(format!(
                    "provider operation {} returned a task, but this adapter is synchronous-only",
                    quote(operation)
                ));
            }
        }
        Ok(result)
    }
    pub async fn install_manifest(&self, ctx: &RunCtx) -> Result<InstallManifest, String> {
        let result = self
            .call(ctx, "opute.provider.get_install_manifest", None, "")
            .await?;
        let content = result.get("structuredContent");
        if result.get("isError") == Some(&Value::Bool(true)) || content.is_none_or(Value::is_null) {
            return Err("provider install manifest call failed".into());
        }
        let mut bytes = String::new();
        crate::gojson::encode(content.unwrap(), &mut bytes);
        let node = crate::gojson::parse(bytes.as_bytes()).expect("encoded JSON");
        let manifest =
            decode_manifest(&node).map_err(|e| format!("decode provider install manifest: {e}"))?;
        let expected = super::ProviderRef {
            id: self.descriptor.plugin_id.clone(),
            version: self.descriptor.version.clone(),
        };
        validate_install_manifest(&manifest, &expected)?;
        Ok(manifest)
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

    #[tokio::test(flavor = "current_thread")]
    async fn close_waits_for_an_accepted_call_then_refuses_new_work() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let provider = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).unwrap() > 0);
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let adapter = std::sync::Arc::new(Adapter {
            descriptor: PluginDescriptor::default(),
            client: Client::new(
                format!("http://{address}/mcp"),
                String::new(),
                String::new(),
                String::new(),
            )
            .unwrap(),
            tools: BTreeSet::from(["test.operation".into()]),
            closed: RwLock::new(false),
        });
        let calling = adapter.clone();
        let call = tokio::spawn(async move {
            calling
                .call(&RunCtx::background(), "test.operation", None, "")
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), adapter.close())
                .await
                .is_err()
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            call.await.unwrap().unwrap(),
            json!({"content":[]}).as_object().unwrap().clone()
        );
        adapter.close().await;
        assert_eq!(
            adapter
                .call(&RunCtx::background(), "test.operation", None, "")
                .await
                .unwrap_err(),
            "provider adapter is closed"
        );
        provider.join().unwrap();
    }

    #[test]
    #[ignore = "requires the pinned client cancellation differential driver"]
    fn cancellation_fixture() {
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_CANCEL_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let base = RunCtx::background();
            let (ctx, cancel) = if let Some(ms) = input["timeoutMs"].as_u64() {
                base.with_timeout(Duration::from_millis(ms))
            } else {
                base.with_cancel()
            };
            if input["before"] == true {
                cancel.cancel();
            }
            let trigger = input["trigger"].as_str().unwrap().to_string();
            let watcher = std::thread::spawn({
                let ctx = ctx.clone();
                let cancel = cancel.clone();
                move || {
                    while ctx.err().is_none() {
                        if std::path::Path::new(&trigger).exists() {
                            cancel.cancel();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            });
            let mut client = Client::new(
                input["endpoint"].as_str().unwrap().into(),
                String::new(),
                "opute-host-agent".into(),
                "dev".into(),
            )
            .unwrap();
            client.progress = Some((
                input["progressStage"].as_str().unwrap().into(),
                input["marker"].as_str().unwrap().into(),
            ));
            let result = client
                .call(&ctx, "server/discover", "", Map::new(), " signed-fixture ")
                .await;
            cancel.cancel();
            watcher.join().unwrap();
            // Let the connection driver observe the dropped body/request future.
            tokio::time::sleep(Duration::from_millis(20)).await;
            match result {
                Ok(result) => json!({"result":result}),
                Err(error) => json!({"error":error}),
            }
        });
        std::fs::write(
            std::env::var("PARITY_PROVIDER_CANCEL_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
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
                Ok(adapter) => {
                    let action = input["action"].as_str().unwrap_or_default();
                    if action.is_empty() {
                        return json!({"tools":adapter.tool_names()});
                    }
                    if action == "closed" {
                        adapter.close().await;
                        adapter.close().await;
                    }
                    let op = input["operation"].as_str().unwrap_or("test.operation");
                    let args = input.get("arguments").and_then(Value::as_object).cloned();
                    let delegation = input["delegation"].as_str().unwrap_or_default();
                    if action == "manifest" {
                        return match adapter.install_manifest(&ctx).await {
                            Ok(m) => json!({"manifest":serde_json::to_value(m).unwrap()}),
                            Err(e) => json!({"error":e}),
                        };
                    }
                    let result = if action == "sync" {
                        adapter
                            .call_synchronous_only(&ctx, op, args, delegation)
                            .await
                    } else {
                        adapter.call(&ctx, op, args, delegation).await
                    };
                    match result {
                        Ok(result) => json!({"result":result}),
                        Err(e) => json!({"error":e}),
                    }
                }
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
