use crate::{
    provider::{
        FunctionCall, HubModelProvider, HubReply, HubRequest, ProviderEvent, ProviderFuture,
        ToolCall,
    },
    Error, Result,
};
use futures_util::StreamExt;
use polyclave_store::Store;
use reqwest::{header, redirect::Policy, Client, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::broadcast;
use zeroize::Zeroizing;

trait Credentials: Send + Sync + 'static {
    fn read(&self, binding: &CredentialBinding) -> Result<Zeroizing<String>>;
}

struct NativeCredentials;
impl Credentials for NativeCredentials {
    fn read(&self, binding: &CredentialBinding) -> Result<Zeroizing<String>> {
        polyclave_os::read_provider_credential(&binding.service, &binding.account)
            .map(Zeroizing::new)
            .map_err(|_| Error::Credential)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialBinding {
    pub service: String,
    pub account: String,
}
impl CredentialBinding {
    /// Changing the registered endpoint requires a distinct credential entry.
    fn bound(id: &str, account: &str, base: &Url) -> Self {
        let scope = format!("{id}\0{base}");
        Self {
            service: format!("Polyclave.Provider.{:x}", Sha256::digest(scope.as_bytes())),
            account: account.into(),
        }
    }
}

pub struct ApiHubProvider {
    client: Client,
    endpoint: Url,
    binding: CredentialBinding,
    models: HashSet<String>,
    credentials: Arc<dyn Credentials>,
}
impl ApiHubProvider {
    pub fn from_registered(store: &Store, id: &str) -> Result<Self> {
        let row=store.read(|db| {
            Ok(db.query_row("SELECT base_url,keyring_account,models_json FROM provider_profiles WHERE id=?1 AND kind='openai_compat'",[id], |r| {
                Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?))
            })?)
        }).map_err(|_| Error::Profile)?;
        let models: Vec<String> = serde_json::from_str(&row.2).map_err(|_| Error::Profile)?;
        Self::new(
            id,
            row.0.as_deref().ok_or(Error::Profile)?,
            row.1.as_deref().ok_or(Error::Profile)?,
            models,
            Arc::new(NativeCredentials),
            false,
        )
    }
    pub fn is_official_deepseek(&self) -> bool {
        self.endpoint.scheme() == "https"
            && self.endpoint.host_str() == Some("api.deepseek.com")
            && self.endpoint.port().is_none()
    }
    pub fn credential_binding(&self) -> &CredentialBinding {
        &self.binding
    }

    fn new(
        id: &str,
        base: &str,
        account: &str,
        models: Vec<String>,
        credentials: Arc<dyn Credentials>,
        _fixture: bool,
    ) -> Result<Self> {
        let mut base = Url::parse(base).map_err(|_| Error::Profile)?;
        let allowed_scheme = base.scheme() == "https"
            || (base.scheme() == "http" && base.host_str() == Some("127.0.0.1"));
        if !allowed_scheme
            || base.host().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || id.is_empty()
            || account.trim().is_empty()
            || models.is_empty()
            || models.iter().any(|m| m.trim().is_empty())
        {
            return Err(Error::Profile);
        }
        let path = base.path().trim_end_matches('/').to_owned();
        let mut segments = base.path_segments_mut().map_err(|_| Error::Profile)?;
        if path != "/" {
            segments.pop_if_empty();
        }
        drop(segments);
        let binding = CredentialBinding::bound(id, account, &base);
        let mut endpoint = base;
        endpoint
            .path_segments_mut()
            .map_err(|_| Error::Profile)?
            .pop_if_empty()
            .push("chat")
            .push("completions");
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| Error::Transport)?;
        Ok(Self {
            client,
            endpoint,
            binding,
            models: models.into_iter().collect(),
            credentials,
        })
    }

    async fn run(
        &self,
        req: HubRequest,
        events: broadcast::Sender<ProviderEvent>,
    ) -> Result<HubReply> {
        if !self.models.contains(&req.model) || req.messages.is_empty() {
            return Err(Error::Profile);
        }
        let credentials = self.credentials.clone();
        let binding = self.binding.clone();
        let secret = tokio::task::spawn_blocking(move || credentials.read(&binding))
            .await
            .map_err(|_| Error::Credential)??;
        if secret.is_empty() {
            return Err(Error::Credential);
        }
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req.tools);
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&**secret)
            .header(header::ACCEPT, "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let detail = read_error_summary(response, &secret).await;
            return Err(Error::Http {
                status: status.as_u16(),
                detail,
            });
        }
        let event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            });
        if !event_stream {
            return Err(Error::Protocol);
        }
        let mut bytes = response.bytes_stream();
        let mut parser = Decoder::default();
        let mut result = Accumulated::default();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(|_| Error::Transport)?;
            for frame in parser.accept(&chunk)? {
                if frame == "[DONE]" {
                    if !result.finished {
                        return Err(Error::Protocol);
                    }
                    let reply = result.finish()?;
                    let _ = events.send(ProviderEvent::Finished);
                    return Ok(reply);
                }
                result.event(&frame, &events)?;
            }
        }
        Err(Error::Protocol)
    }
}
impl HubModelProvider for ApiHubProvider {
    fn complete(
        &self,
        req: HubRequest,
        events: broadcast::Sender<ProviderEvent>,
    ) -> ProviderFuture<'_> {
        Box::pin(self.run(req, events))
    }
}

const FRAME_LIMIT: usize = 256 * 1024;
const RESULT_LIMIT: usize = 2 * 1024 * 1024;

fn sanitize_summary(raw: &str, secret: Option<&str>) -> String {
    let mut text = raw.trim().replace(['\r', '\n'], " ");
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    let end = text.floor_char_boundary(text.len().min(256));
    text.truncate(end);
    text
}

fn summarize_error_value(err: &Value) -> String {
    let raw = err["message"]
        .as_str()
        .or_else(|| err.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| err.to_string());
    sanitize_summary(&raw, None)
}

async fn read_error_summary(response: reqwest::Response, secret: &str) -> String {
    let mut stream = response.bytes_stream();
    let mut buf = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        let take = (4096 - buf.len()).min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= 4096 {
            break;
        }
    }
    if buf.is_empty() {
        return String::new();
    }
    if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
        if let Some(msg) = v["error"]["message"]
            .as_str()
            .or_else(|| v["message"].as_str())
        {
            return sanitize_summary(msg, Some(secret));
        }
    }
    sanitize_summary(&String::from_utf8_lossy(&buf), Some(secret))
}

#[derive(Default)]
struct Decoder {
    line: Vec<u8>,
    data: String,
}
impl Decoder {
    fn accept(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for &b in bytes {
            if b != b'\n' {
                if self.line.len() >= FRAME_LIMIT {
                    return Err(Error::Limit);
                }
                self.line.push(b);
                continue;
            }
            let line = std::str::from_utf8(&self.line)
                .map_err(|_| Error::Protocol)?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(std::mem::take(&mut self.data));
                }
            } else if let Some(field) = line.strip_prefix("data:") {
                let field = field.strip_prefix(' ').unwrap_or(field);
                if self.data.len() + field.len() + 1 > FRAME_LIMIT {
                    return Err(Error::Limit);
                }
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(field);
            }
            self.line.clear();
        }
        Ok(out)
    }
}

#[derive(Default)]
struct CallParts {
    id: String,
    name: String,
    arguments: String,
}
#[derive(Default)]
struct Accumulated {
    reply: HubReply,
    calls: BTreeMap<u32, CallParts>,
    size: usize,
    finished: bool,
}
impl Accumulated {
    fn event(&mut self, frame: &str, events: &broadcast::Sender<ProviderEvent>) -> Result<()> {
        self.size = self
            .size
            .checked_add(frame.len())
            .filter(|n| *n <= RESULT_LIMIT)
            .ok_or(Error::Limit)?;
        let v: Value = serde_json::from_str(frame).map_err(|_| Error::Protocol)?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let status = err["code"]
                .as_u64()
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(502);
            let detail = summarize_error_value(err);
            return Err(Error::Http { status, detail });
        }
        if let Some(usage) = v.get("usage").filter(|u| u.is_object()) {
            self.reply.usage = Some(json!({
                "total_tokens":usage["total_tokens"].as_u64(),
                "prompt_tokens":usage["prompt_tokens"].as_u64(),
                "completion_tokens":usage["completion_tokens"].as_u64(),
                "cost":usage["cost"].as_f64().filter(|v|*v>=0.)
            }));
        }
        let Some(choices_val) = v.get("choices").filter(|c| !c.is_null()) else {
            return Ok(());
        };
        let choices = choices_val.as_array().ok_or(Error::Protocol)?;
        if choices.is_empty() {
            return Ok(());
        }
        if choices.len() != 1 || choices[0]["index"] != 0 || self.finished {
            return Err(Error::Protocol);
        }
        let c = &choices[0];
        let d = &c["delta"];
        if let Some(text) = d["content"].as_str() {
            self.reply.text.push_str(text);
            let _ = events.send(ProviderEvent::Text(text.into()));
        }
        if let Some(thought) = d["reasoning_content"].as_str() {
            self.reply.reasoning.push_str(thought);
            let _ = events.send(ProviderEvent::Reasoning(thought.into()));
        }
        if let Some(parts) = d["tool_calls"].as_array() {
            for p in parts {
                let index = p["index"]
                    .as_u64()
                    .filter(|i| *i < 64)
                    .ok_or(Error::Protocol)? as u32;
                if p["type"].as_str().is_some_and(|t| t != "function") {
                    return Err(Error::Protocol);
                }
                let call = self.calls.entry(index).or_default();
                if let Some(id) = p["id"].as_str().filter(|s| !s.is_empty()) {
                    if call.id.is_empty() {
                        call.id.push_str(id);
                    } else if call.id != id {
                        if id.starts_with(&call.id) {
                            call.id = id.to_owned();
                        } else {
                            call.id.push_str(id);
                        }
                    }
                }
                if let Some(name) = p["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                    if call.name.is_empty() {
                        call.name.push_str(name);
                    } else if call.name != name {
                        if name.starts_with(&call.name) {
                            call.name = name.to_owned();
                        } else {
                            call.name.push_str(name);
                        }
                    }
                }
                if let Some(arguments) = p["function"]["arguments"].as_str() {
                    call.arguments.push_str(arguments);
                    let _ = events.send(ProviderEvent::ToolDelta {
                        index,
                        arguments: arguments.into(),
                    });
                }
            }
        }
        if let Some(reason) = c["finish_reason"].as_str() {
            if !reason.is_empty() {
                if !matches!(
                    reason,
                    "stop" | "tool_calls" | "length" | "content_filter" | "function_call"
                ) {
                    return Err(Error::Protocol);
                }
                self.finished = true;
            }
        }
        Ok(())
    }
    fn finish(mut self) -> Result<HubReply> {
        let mut ids = HashSet::new();
        for mut parts in self.calls.into_values() {
            if parts.arguments.trim().is_empty() {
                parts.arguments = "{}".into();
            }
            if parts.id.is_empty()
                || parts.name.is_empty()
                || !ids.insert(parts.id.clone())
                || !serde_json::from_str::<Value>(&parts.arguments).is_ok_and(|v| v.is_object())
            {
                return Err(Error::Protocol);
            }
            self.reply.calls.push(ToolCall {
                id: parts.id,
                kind: "function".into(),
                function: FunctionCall {
                    name: parts.name,
                    arguments: parts.arguments,
                },
            });
        }
        Ok(self.reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{assemble, ChatMessage, Role};
    use axum::{
        body::Body,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Redirect},
        routing::post,
        Json, Router,
    };
    use std::convert::Infallible;

    struct FakeCredential;
    impl Credentials for FakeCredential {
        fn read(&self, _: &CredentialBinding) -> Result<Zeroizing<String>> {
            Ok(Zeroizing::new("fixture-bearer-value".into()))
        }
    }

    async fn llm(headers: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(headers["authorization"], "Bearer fixture-bearer-value");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hello");
        assert_eq!(body["tools"][0]["function"]["name"], "list_repo_files");
        let mut packet = Vec::new();
        for frame in [
            json!({"choices":[{"index":0,"delta":{"reasoning_content":"考察","content":"Hello "}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"world","tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"list_repo_files","arguments":"{\"dir\":"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"src\"}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"total_tokens":3}}),
        ] {
            packet.extend(format!("data: {frame}\r\n\r\n").bytes());
        }
        packet.extend(b"data: [DONE]\r\n\r\n");
        let chunks = packet
            .chunks(3)
            .map(|b| Ok::<_, Infallible>(b.to_vec()))
            .collect::<Vec<_>>();
        (
            [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
            Body::from_stream(futures_util::stream::iter(chunks)),
        )
    }
    fn request(model: &str) -> HubRequest {
        HubRequest {
            model: model.into(),
            messages: vec![
                ChatMessage::text(Role::System, "system"),
                ChatMessage::text(Role::User, "hello"),
            ],
            tools: vec![
                json!({"type":"function","function":{"name":"list_repo_files","parameters":{"type":"object"}}}),
            ],
        }
    }
    async fn fake_server(router: Router) -> (String, tokio::task::JoinHandle<std::io::Result<()>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (
            format!("http://{addr}/v1/"),
            tokio::spawn(async move { axum::serve(listener, router).await }),
        )
    }

    #[tokio::test]
    async fn fake_llm_streams_reasoning_and_native_tool_calls_across_packet_boundaries() {
        let (url, task) = fake_server(Router::new().route("/v1/chat/completions", post(llm))).await;
        let p = ApiHubProvider::new(
            "fake",
            &url,
            "test",
            vec!["model".into()],
            Arc::new(FakeCredential),
            true,
        )
        .unwrap();
        let (tx, mut rx) = broadcast::channel(16);
        assert!(matches!(
            p.complete(request("not-allowlisted"), tx.clone()).await,
            Err(Error::Profile)
        ));
        let reply = p.complete(request("model"), tx).await.unwrap();
        assert_eq!(reply.reasoning, "考察");
        assert_eq!(reply.text, "Hello world");
        assert_eq!(reply.calls.len(), 1);
        assert_eq!(reply.calls[0].function.arguments, r#"{"dir":"src"}"#);
        assert_eq!(rx.try_recv().unwrap(), ProviderEvent::Text("Hello ".into()));
        assert_eq!(
            rx.try_recv().unwrap(),
            ProviderEvent::Reasoning("考察".into())
        );
        let mut count = 0;
        while let Ok(event) = rx.try_recv() {
            if event == ProviderEvent::Finished {
                count += 1;
            }
        }
        assert_eq!(count, 1);
        task.abort();
    }

    #[tokio::test]
    async fn redirects_never_receive_a_second_request_and_truncation_never_finishes() {
        let (url, task) = fake_server(
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(|| async { Redirect::temporary("/redirect") }),
                )
                .route(
                    "/redirect",
                    post(|| async {
                        panic!("redirect was followed");
                        #[allow(unreachable_code)]
                        StatusCode::OK
                    }),
                ),
        )
        .await;
        let p = ApiHubProvider::new(
            "fake",
            &url,
            "test",
            vec!["model".into()],
            Arc::new(FakeCredential),
            true,
        )
        .unwrap();
        let (tx, mut rx) = broadcast::channel(16);
        assert!(matches!(
            p.complete(request("model"), tx.clone()).await,
            Err(Error::Http { status: 307, .. })
        ));
        assert!(rx.try_recv().is_err());
        task.abort();

        let (url, task) = fake_server(Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    "data: [DONE]\n\n",
                )
            }),
        ))
        .await;
        let p = ApiHubProvider::new(
            "fake",
            &url,
            "test",
            vec!["model".into()],
            Arc::new(FakeCredential),
            true,
        )
        .unwrap();
        assert!(matches!(
            p.complete(request("model"), tx).await,
            Err(Error::Protocol)
        ));
        assert!(rx.try_recv().is_err());
        task.abort();
    }

    #[test]
    fn prefix_is_stable_and_the_key_identity_is_endpoint_bound() {
        let input = ChatMessage::text(Role::User, "current");
        let assistant = ChatMessage::text(Role::Assistant, "prior");
        let result = assemble(
            "system",
            &[],
            "summary",
            std::slice::from_ref(&assistant),
            &json!({"status":"running"}),
            input.clone(),
        )
        .unwrap();
        assert_eq!(result.len(), 5);
        assert!(result[0].content.contains("<project_memory_brief>"));
        assert!(result[1].content.contains("summary"));
        assert_eq!(result[2], assistant);
        assert!(result[3].content.contains("running"));
        assert_eq!(result[4], input);
        let key =
            |url| CredentialBinding::bound("one", "account", &Url::parse(url).unwrap()).service;
        assert_ne!(
            key("https://one.example.invalid/v1"),
            key("https://two.example.invalid/v1")
        );
        assert_ne!(
            key("https://one.example.invalid/v1"),
            key("https://one.example.invalid/v2")
        );
    }
    #[tokio::test]
    async fn later_flash_call_replays_reasoning_from_the_durable_first_turn() {
        use crate::provider::restore_api_history;
        use polyclave_store::HubCompletion;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flash.db");
        let store = Store::open(&path).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "
                INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                    VALUES('flash','Flash','openai_compat','[]','now');
                INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                    VALUES('hub','Hub','hub','repo','flash','deepseek-flash','now');",
                )?;
                Ok(())
            })
            .unwrap();
        async fn endpoint(Json(v): Json<Value>) -> impl IntoResponse {
            let second = v["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["content"] == "continue");
            assert!(v["tools"].is_array());
            let prior = v["messages"].as_array().unwrap().iter().any(|m| {
                m["role"] == "assistant"
                    && m["content"] == "first answer"
                    && m["reasoning_content"] == "recorded prior reasoning"
            });
            if second && !prior {
                return (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    String::new(),
                );
            }
            let data = if second {
                json!({"content":"second answer"})
            } else {
                json!({"content":"first answer","reasoning_content":"recorded prior reasoning"})
            };
            let frame = json!({"choices":[{"index":0,"delta":data,"finish_reason":"stop"}]});
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {frame}\n\ndata: [DONE]\n\n"),
            )
        }
        let (url, server) =
            fake_server(Router::new().route("/v1/chat/completions", post(endpoint))).await;
        let p = ApiHubProvider::new(
            "flash",
            &url,
            "test",
            vec!["deepseek-flash".into()],
            Arc::new(FakeCredential),
            true,
        )
        .unwrap();
        let (events, _) = broadcast::channel(8);
        store.hub_enqueue("hub", "one", "begin").unwrap();
        let turn = store.hub_claim("hub", "fixture", "boot").unwrap().unwrap();
        store.hub_authorize(turn.id, None).unwrap();
        let reply = p
            .complete(request("deepseek-flash"), events.clone())
            .await
            .unwrap();
        assert!(reply.calls.is_empty());
        let mut message = ChatMessage::text(Role::Assistant, &reply.text);
        message.reasoning_content = Some(reply.reasoning.clone());
        store
            .hub_finish_api(
                turn,
                HubCompletion {
                    text: reply.text,
                    reasoning: Some(reply.reasoning),
                    ..Default::default()
                },
                json!([message]),
            )
            .unwrap();
        drop(store);
        let store = Store::open(path).unwrap();
        store.hub_enqueue("hub", "two", "continue").unwrap();
        let turn = store.hub_claim("hub", "fixture", "boot").unwrap().unwrap();
        let history = restore_api_history(
            store
                .hub_api_history("hub", "flash", turn.input.id)
                .unwrap(),
            true,
        )
        .unwrap();
        let cross = restore_api_history(
            store
                .hub_api_history("hub", "other", turn.input.id)
                .unwrap(),
            false,
        )
        .unwrap();
        assert!(cross.iter().all(|item| item.reasoning_content.is_none()));
        let mut second = request("deepseek-flash");
        second.messages = history;
        second
            .messages
            .push(ChatMessage::text(Role::User, "continue"));
        assert_eq!(
            p.complete(second, events).await.unwrap().text,
            "second answer"
        );
        assert!(store
            .hub_messages("hub")
            .unwrap()
            .iter()
            .all(|m| m.metadata.is_none()));
        server.abort();
    }

    #[tokio::test]
    async fn sse_handles_repeated_tool_metadata_empty_args_usage_only_frames_and_http_errors() {
        async fn compat_llm(Json(body): Json<Value>) -> impl IntoResponse {
            assert_eq!(body["stream_options"]["include_usage"], true);
            let frames = [
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"memory_brief","arguments":""}}]},"finish_reason":""}]}),
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"memory_brief","arguments":""}}]},"finish_reason":"length"}]}),
                json!({"usage":{"total_tokens":10,"prompt_tokens":6,"completion_tokens":4}}),
            ];
            let mut packet = String::new();
            for f in frames {
                packet.push_str(&format!("data: {f}\n\n"));
            }
            packet.push_str("data: [DONE]\n\n");
            (
                [(header::CONTENT_TYPE, " Text/Event-Stream ; charset=utf-8")],
                packet,
            )
        }
        let (url, task) =
            fake_server(Router::new().route("/v1/chat/completions", post(compat_llm))).await;
        let p = ApiHubProvider::new(
            "compat",
            &url,
            "test",
            vec!["model".into()],
            Arc::new(FakeCredential),
            false,
        )
        .unwrap();
        let (tx, _) = broadcast::channel(16);
        let reply = p.complete(request("model"), tx.clone()).await.unwrap();
        assert_eq!(reply.calls.len(), 1);
        assert_eq!(reply.calls[0].id, "call_1");
        assert_eq!(reply.calls[0].function.name, "memory_brief");
        assert_eq!(reply.calls[0].function.arguments, "{}");
        assert_eq!(reply.usage.as_ref().unwrap()["total_tokens"], 10);
        task.abort();

        let (err_url, err_task) = fake_server(Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error":{"message":"rate limit hit"}})),
                )
            }),
        ))
        .await;
        let p = ApiHubProvider::new(
            "compat",
            &err_url,
            "test",
            vec!["model".into()],
            Arc::new(FakeCredential),
            false,
        )
        .unwrap();
        match p.complete(request("model"), tx).await {
            Err(Error::Http { status, detail }) => {
                assert_eq!(status, 429);
                assert!(detail.contains("rate limit hit"));
            }
            other => panic!("expected Http 429, got {other:?}"),
        }
        err_task.abort();
    }
}
