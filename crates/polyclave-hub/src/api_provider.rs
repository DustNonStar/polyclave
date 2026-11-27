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
    pub fn credential_binding(&self) -> &CredentialBinding {
        &self.binding
    }

    fn new(
        id: &str,
        base: &str,
        account: &str,
        models: Vec<String>,
        credentials: Arc<dyn Credentials>,
        fixture: bool,
    ) -> Result<Self> {
        let mut base = Url::parse(base).map_err(|_| Error::Profile)?;
        let allowed_scheme = base.scheme() == "https"
            || (cfg!(test)
                && fixture
                && base.scheme() == "http"
                && base.host_str() == Some("127.0.0.1"));
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
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
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
        let mut body = json!({"model":req.model,"messages":req.messages,"stream":true});
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
        let event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("text/event-stream"));
        if !response.status().is_success() || !event_stream {
            return Err(Error::Transport);
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
        if v.get("error").is_some() {
            return Err(Error::Transport);
        }
        let choices = v["choices"].as_array().ok_or(Error::Protocol)?;
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
                if let Some(id) = p["id"].as_str() {
                    call.id.push_str(id);
                }
                if let Some(name) = p["function"]["name"].as_str() {
                    call.name.push_str(name);
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
            if !matches!(reason, "stop" | "tool_calls") {
                return Err(Error::Protocol);
            }
            self.finished = true;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<HubReply> {
        let mut ids = HashSet::new();
        for parts in self.calls.into_values() {
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
            Err(Error::Transport)
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
}
