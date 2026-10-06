use super::{config::endpoint, types::*};
use futures_util::StreamExt;
use promptify_core::{
    pipeline::{BackendError, CancelToken, FinishReason, Generation, GenerationRequest, Generator},
    prompt::{ChatMessage, Role},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_FRAME: usize = 256 * 1024;
const MAX_TEXT: usize = 256 * 1024;
fn error(message: &str) -> BackendError {
    BackendError(message.into())
}

#[derive(Default)]
pub struct SessionControl {
    revoked: AtomicBool,
    cancel: Mutex<Option<CancelToken>>,
}
impl SessionControl {
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        if let Some(cancel) = self.cancel.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    }
    fn remember(&self, cancel: &CancelToken) {
        let mut token = self.cancel.lock().unwrap();
        if self.is_revoked() {
            cancel.cancel();
        }
        *token = Some(cancel.clone());
    }
}
pub struct HttpSession {
    pub connection: InferenceConnection,
    pub model: String,
    pub secret: Option<String>,
    pub revoked: Arc<SessionControl>,
}
fn role(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}
fn body(c: &InferenceConnection, model: &str, messages: &[ChatMessage], max: u32) -> Value {
    let turns: Vec<_> = messages
        .iter()
        .map(|m| json!({"role":role(m.role),"content":m.content}))
        .collect();
    match c.protocol {
        Protocol::OpenaiChatCompletions => {
            let mut value = json!({"model":model,"messages":turns,"stream":c.stream});
            value[if c.provider == Provider::Openai {
                "max_completion_tokens"
            } else {
                "max_tokens"
            }] = json!(max);
            value
        }
        Protocol::OpenaiResponses => {
            json!({"model":model,"input":turns,"max_output_tokens":max,"stream":c.stream,"store":false})
        }
        Protocol::AnthropicMessages => {
            let system: Vec<_> = messages
                .iter()
                .filter(|m| m.role == Role::System)
                .map(|m| json!({"type":"text","text":m.content}))
                .collect();
            let turns: Vec<_> = messages
                .iter()
                .filter(|m| m.role != Role::System)
                .map(|m| json!({"role":role(m.role),"content":m.content}))
                .collect();
            json!({"model":model,"system":system,"messages":turns,"max_tokens":max,"stream":c.stream})
        }
    }
}
fn client(c: &InferenceConnection) -> Result<reqwest::Client, BackendError> {
    let url = endpoint(c, "models").map_err(BackendError)?;
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10));
    let builder = if loopback {
        builder.no_proxy()
    } else {
        builder
    };
    builder
        .build()
        .map_err(|_| error("Could not initialize secure inference transport."))
}
fn authorize(
    request: reqwest::RequestBuilder,
    c: &InferenceConnection,
    secret: Option<&str>,
) -> Result<reqwest::RequestBuilder, BackendError> {
    let request = if c.protocol == Protocol::AnthropicMessages {
        request.header("anthropic-version", "2023-06-01")
    } else {
        request
    };
    if c.auth == AuthMode::None {
        return Ok(request);
    }
    let secret = secret
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("API credential missing. Save a key in Inference settings."))?;
    Ok(if c.protocol == Protocol::AnthropicMessages {
        request.header("x-api-key", secret)
    } else {
        request.bearer_auth(secret)
    })
}
fn check_status(response: &reqwest::Response) -> Result<(), BackendError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let category = match status.as_u16() {
        401 | 403 => "authentication or permission denied",
        404 => "endpoint or model unavailable",
        408 | 504 => "endpoint timed out",
        429 => "rate limit or quota exceeded",
        400 | 422 => "unsupported request, model, or context limit",
        300..=399 => "redirect refused; configure the final API base URL",
        500..=599 => "provider or model loading failure",
        _ => "request rejected",
    };
    let request_id = ["x-request-id", "request-id"]
        .iter()
        .find_map(|name| response.headers().get(*name).and_then(|v| v.to_str().ok()))
        .filter(|id| {
            id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        });
    let request_id = request_id
        .map(|id| format!(" Request ID: {id}."))
        .unwrap_or_default();
    Err(BackendError(format!(
        "Inference HTTP {}: {category}.{request_id} No retry or fallback was attempted.",
        status.as_u16()
    )))
}
enum Event {
    Token(String),
    Done(Result<Generation, BackendError>),
}
impl Generator for HttpSession {
    fn is_valid(&self) -> bool {
        !self.revoked.is_revoked()
    }

    fn generate(
        &self,
        request: &GenerationRequest<'_>,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generation, BackendError> {
        self.revoked.remember(cancel);
        if cancel.is_cancelled() || self.revoked.is_revoked() {
            return Err(error("Inference session cancelled or credential revoked."));
        }
        if Instant::now() >= request.deadline {
            return Err(error("Inference deadline exceeded."));
        }
        let connection = self.connection.clone();
        let body = body(
            &connection,
            &self.model,
            request.messages,
            request.max_new_tokens,
        );
        let secret = self.secret.clone();
        let revoked = self.revoked.clone();
        let caller_cancel = cancel.clone();
        let cancel = cancel.clone();
        let deadline = request.deadline;
        let (tx, rx) = mpsc::channel();
        let thread=std::thread::Builder::new().name("inference-http".into()).spawn(move||{
            let result=tokio::runtime::Builder::new_current_thread().enable_all().build()
                .map_err(|_|error("Could not start inference runtime."))
                .and_then(|runtime|runtime.block_on(async {
                    let work=generate_http(&connection,secret.as_deref(),body,deadline,&tx);
                    tokio::pin!(work);
                    let mut interval=tokio::time::interval(Duration::from_millis(20));
                    loop {
                        tokio::select! {
                            result=&mut work=>break result,
                            _=interval.tick()=>{
                                if cancel.is_cancelled()||revoked.is_revoked() {break Err(error("Inference session cancelled or credential revoked."));}
                                if Instant::now()>=deadline {break Err(error("Inference deadline exceeded."));}
                            }
                        }
                    }
                }));
            let _=tx.send(Event::Done(result));
        }).map_err(|_|error("Could not start inference transport thread."))?;
        let result = loop {
            match rx.recv() {
                Ok(Event::Token(text)) => on_token(&text),
                Ok(Event::Done(result)) => break result,
                Err(_) => break Err(error("Inference transport stopped unexpectedly.")),
            }
        };
        let _ = thread.join();
        if caller_cancel.is_cancelled() || self.revoked.is_revoked() {
            return Err(error("Inference session cancelled or credential revoked."));
        }
        result
    }
}
async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, BackendError> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| error("Inference connection interrupted."))?;
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(error("Inference response exceeds size limit."));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn generate_http(
    c: &InferenceConnection,
    secret: Option<&str>,
    body: Value,
    deadline: Instant,
    tx: &mpsc::Sender<Event>,
) -> Result<Generation, BackendError> {
    let path = match c.protocol {
        Protocol::OpenaiChatCompletions => "chat/completions",
        Protocol::OpenaiResponses => "responses",
        Protocol::AnthropicMessages => "messages",
    };
    let request = client(c)?
        .post(endpoint(c, path).map_err(BackendError)?)
        .timeout(deadline.saturating_duration_since(Instant::now()))
        .json(&body);
    let response=authorize(request,c,secret)?.send().await.map_err(|_|error("Cannot reach inference endpoint or request timed out. Check server, port, TLS, and deadline."))?;
    check_status(&response)?;
    if !c.stream {
        let value: Value = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| error("Malformed inference response."))?;
        let generation = parse_nonstream(c.protocol, &value)?;
        let _ = tx.send(Event::Token(generation.text.clone()));
        return Ok(generation);
    }
    let mut parser = Parser::new(c.protocol);
    let mut pending = Vec::new();
    let mut stream = response.bytes_stream();
    let mut total = 0;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| error("Inference stream interrupted; output is incomplete."))?;
        total += chunk.len();
        if total > MAX_BODY {
            return Err(error("Inference stream exceeds size limit."));
        }
        pending.extend_from_slice(&chunk);
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            if pos > MAX_FRAME {
                return Err(error("Inference frame exceeds size limit."));
            }
            let line: Vec<_> = pending.drain(..=pos).collect();
            let line = std::str::from_utf8(&line)
                .map_err(|_| error("Invalid UTF-8 inference stream."))?
                .trim_end_matches(['\r', '\n']);
            for token in parser.line(line)? {
                let _ = tx.send(Event::Token(token));
            }
            if parser.terminal {
                return parser.finish();
            }
        }
        if pending.len() > MAX_FRAME {
            return Err(error("Inference frame exceeds size limit."));
        }
    }
    Err(error(
        "Inference stream ended without a valid terminal event; output is incomplete.",
    ))
}
fn finish_reason(reason: &str) -> Result<FinishReason, BackendError> {
    match reason {
        "stop" | "end_turn" | "stop_sequence" | "completed" => Ok(FinishReason::Stop),
        "length" | "max_tokens" | "max_output_tokens" => Ok(FinishReason::Length),
        _ => Err(error(
            "Inference refused, requested a tool, or returned unsupported termination.",
        )),
    }
}
fn response_text(value: &Value) -> Result<String, BackendError> {
    let mut text = String::new();
    for output in value["output"]
        .as_array()
        .ok_or_else(|| error("Missing Responses output."))?
    {
        match output["type"].as_str() {
            Some("reasoning") => continue,
            Some("message") if output["role"] == "assistant" => {
                for block in output["content"]
                    .as_array()
                    .ok_or_else(|| error("Missing Responses content."))?
                {
                    if block["type"] != "output_text" {
                        return Err(error("Unsupported or refused Responses content."));
                    }
                    text.push_str(
                        block["text"]
                            .as_str()
                            .ok_or_else(|| error("Missing Responses text."))?,
                    );
                }
            }
            _ => return Err(error("Unsupported Responses tool or non-text output.")),
        }
    }
    Ok(text)
}
fn parse_nonstream(protocol: Protocol, value: &Value) -> Result<Generation, BackendError> {
    if !value["error"].is_null() {
        return Err(error("Provider returned an inference error."));
    }
    let (text, finish) = match protocol {
        Protocol::OpenaiChatCompletions => {
            let choices = value["choices"]
                .as_array()
                .filter(|v| v.len() == 1)
                .ok_or_else(|| error("Expected one Chat Completions choice."))?;
            let choice = &choices[0];
            if !choice["message"]["refusal"].is_null()
                || !choice["message"]["tool_calls"].is_null()
                || !choice["message"]["function_call"].is_null()
            {
                return Err(error("Provider refused or requested a tool."));
            }
            (
                choice["message"]["content"]
                    .as_str()
                    .ok_or_else(|| error("Missing text completion."))?
                    .to_owned(),
                finish_reason(choice["finish_reason"].as_str().unwrap_or(""))?,
            )
        }
        Protocol::OpenaiResponses => {
            let reason = match value["status"].as_str() {
                Some("completed") => "completed",
                Some("incomplete") => value["incomplete_details"]["reason"].as_str().unwrap_or(""),
                _ => return Err(error("Responses result did not complete.")),
            };
            (response_text(value)?, finish_reason(reason)?)
        }
        Protocol::AnthropicMessages => {
            let mut text = String::new();
            for block in value["content"]
                .as_array()
                .ok_or_else(|| error("Missing Anthropic content."))?
            {
                match block["type"].as_str() {
                    Some("text") => text.push_str(
                        block["text"]
                            .as_str()
                            .ok_or_else(|| error("Missing Anthropic text."))?,
                    ),
                    Some("thinking" | "redacted_thinking") => {}
                    _ => return Err(error("Unsupported Anthropic tool or non-text output.")),
                }
            }
            (
                text,
                finish_reason(value["stop_reason"].as_str().unwrap_or(""))?,
            )
        }
    };
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err(error("Inference output is empty or exceeds size limit."));
    }
    Ok(Generation { text, finish })
}
struct Parser {
    protocol: Protocol,
    data: String,
    text: String,
    reason: Option<FinishReason>,
    terminal: bool,
}
impl Parser {
    fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            data: String::new(),
            text: String::new(),
            reason: None,
            terminal: false,
        }
    }
    fn line(&mut self, line: &str) -> Result<Vec<String>, BackendError> {
        if line.is_empty() {
            if self.data.is_empty() {
                return Ok(vec![]);
            }
            let data = std::mem::take(&mut self.data);
            return self.event(data.trim_end_matches('\n'));
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            self.data.push('\n');
            if self.data.len() > MAX_FRAME {
                return Err(error("Inference event exceeds size limit."));
            }
        }
        Ok(vec![])
    }
    fn event(&mut self, data: &str) -> Result<Vec<String>, BackendError> {
        if data == "[DONE]" {
            if self.protocol != Protocol::OpenaiChatCompletions || self.reason.is_none() {
                return Err(error("Premature inference terminal marker."));
            }
            self.terminal = true;
            return Ok(vec![]);
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| error("Malformed inference event."))?;
        if !value["error"].is_null() || value["type"] == "error" {
            return Err(error("Provider returned an inference error."));
        }
        let token = match self.protocol {
            Protocol::OpenaiChatCompletions => {
                let choices = value["choices"]
                    .as_array()
                    .ok_or_else(|| error("Missing streamed choices."))?;
                if choices.is_empty() {
                    return Ok(vec![]);
                }
                if choices.len() != 1 || choices[0]["index"].as_u64().unwrap_or(0) != 0 {
                    return Err(error("Unexpected streamed choice."));
                }
                let choice = &choices[0];
                if !choice["delta"]["refusal"].is_null()
                    || !choice["delta"]["tool_calls"].is_null()
                    || !choice["delta"]["function_call"].is_null()
                {
                    return Err(error("Provider refused or requested a tool."));
                }
                if self.reason.is_some()
                    && choice["delta"]["content"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
                {
                    return Err(error("Text received after completion finish reason."));
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.reason = Some(finish_reason(reason)?);
                }
                choice["delta"]["content"].as_str().unwrap_or("").to_owned()
            }
            Protocol::OpenaiResponses => match value["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => value["delta"]
                    .as_str()
                    .ok_or_else(|| error("Missing Responses delta."))?
                    .to_owned(),
                "response.completed" | "response.incomplete" => {
                    let generation =
                        parse_nonstream(Protocol::OpenaiResponses, &value["response"])?;
                    if !self.text.is_empty() && self.text != generation.text {
                        return Err(error("Responses terminal text does not match its stream."));
                    }
                    let token = if self.text.is_empty() {
                        generation.text
                    } else {
                        String::new()
                    };
                    self.reason = Some(generation.finish);
                    self.terminal = true;
                    token
                }
                "response.failed"
                | "response.error"
                | "response.refusal.delta"
                | "response.refusal.done"
                | "response.function_call_arguments.delta" => {
                    return Err(error("Responses failed, refused, or requested a tool."));
                }
                "response.output_item.added"
                    if !matches!(
                        value["item"]["type"].as_str(),
                        Some("message" | "reasoning")
                    ) =>
                {
                    return Err(error("Unsupported Responses output item."));
                }
                _ => String::new(),
            },
            Protocol::AnthropicMessages => match value["type"].as_str().unwrap_or("") {
                "content_block_start" => match value["content_block"]["type"].as_str() {
                    Some("text") => value["content_block"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                    Some("thinking" | "redacted_thinking") => String::new(),
                    _ => return Err(error("Unsupported Anthropic content block.")),
                },
                "content_block_delta" => match value["delta"]["type"].as_str() {
                    Some("text_delta") => value["delta"]["text"]
                        .as_str()
                        .ok_or_else(|| error("Missing Anthropic text delta."))?
                        .to_owned(),
                    Some("thinking_delta" | "signature_delta") => String::new(),
                    _ => return Err(error("Unsupported Anthropic content delta.")),
                },
                "message_delta" => {
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        self.reason = Some(finish_reason(reason)?);
                    }
                    String::new()
                }
                "message_stop" => {
                    self.terminal = true;
                    String::new()
                }
                "message_start" | "content_block_stop" | "ping" => String::new(),
                _ => return Err(error("Unknown Anthropic stream event.")),
            },
        };
        if token.is_empty() {
            return Ok(vec![]);
        }
        if self.text.len() + token.len() > MAX_TEXT {
            return Err(error("Inference output exceeds size limit."));
        }
        self.text.push_str(&token);
        Ok(vec![token])
    }
    fn finish(self) -> Result<Generation, BackendError> {
        if self.text.is_empty() {
            return Err(error("Inference returned no visible text."));
        }
        Ok(Generation {
            text: self.text,
            finish: self
                .reason
                .ok_or_else(|| error("Missing inference finish reason."))?,
        })
    }
}
fn discovery_endpoint(c: &InferenceConnection) -> Result<(reqwest::Url, bool), BackendError> {
    let mut url = endpoint(c, "models").map_err(BackendError)?;
    // The Anthropic metadata route is filtered; only the VS Code catalogue includes all vendors.
    if c.provider == Provider::Custom
        && let Some(prefix) = url.path().strip_suffix("/api/openai/v1/models")
            .or_else(|| url.path().strip_suffix("/api/anthropic/v1/models"))
    {
        let path = format!("{prefix}/api/v1/lm/chatModels");
        url.set_path(&path);
        return Ok((url, true));
    }
    Ok((url, false))
}

fn discovered_models(value: &Value, maestro: bool) -> Result<Vec<DiscoveredModel>, BackendError> {
    let models = if maestro { value } else { &value["data"] }.as_array()
        .ok_or_else(|| error("Endpoint does not support model discovery. Enter a model manually."))?;
    models.iter()
        // Maestro's generation proxy selects only the copilot vendor, not recursive custom endpoints.
        .filter(|model| !maestro || model["vendor"].as_str() == Some("copilot"))
        .map(|model| {
            let id = model["id"].as_str().filter(|id| !id.trim().is_empty())
                .ok_or_else(|| error("Invalid model discovery response: missing model identifier."))?;
            let name = model[if maestro { "name" } else { "display_name" }].as_str().unwrap_or(id);
            Ok(DiscoveredModel { id: id.into(), name: name.into() })
        }).collect()
}

pub fn discover(
    c: InferenceConnection,
    secret: Option<String>,
    revoked: Arc<SessionControl>,
) -> Result<Vec<DiscoveredModel>, String> {
    std::thread::spawn(move||{
        tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|error("Could not start inference runtime."))?.block_on(async {
            let work=async {
                let (url, maestro) = discovery_endpoint(&c)?;
                let request = client(&c)?.get(url).timeout(Duration::from_secs(15));
                // Maestro's catalogue is outside LLM-key authentication; do not disclose that key.
                let request = if maestro { request } else { authorize(request, &c, secret.as_deref())? };
                let response=request
                    .send().await.map_err(|_|error("Cannot discover models. Check server/port or enter a model manually."))?;
                check_status(&response)?;
                let value:Value=serde_json::from_slice(&bounded_body(response).await?).map_err(|_|error("Invalid model discovery response."))?;
                discovered_models(&value, maestro)
            };
            tokio::pin!(work);
            let mut interval=tokio::time::interval(Duration::from_millis(20));
            loop {
                tokio::select! {
                    result=&mut work=>break result,
                    _=interval.tick()=>if revoked.is_revoked(){break Err(error("Inference connection revoked."));}
                }
            }
        })
    }).join().map_err(|_|"Discovery transport stopped.".to_string())?.map_err(|e|e.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_finish_refusals_and_reasoning() {
        let mut chat = Parser::new(Protocol::OpenaiChatCompletions);
        chat.event(r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#)
            .unwrap();
        assert!(chat.event("[DONE]").is_err());
        chat.event(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#)
            .unwrap();
        chat.event("[DONE]").unwrap();
        assert_eq!(chat.finish().unwrap().finish, FinishReason::Length);
        assert!(finish_reason("tool_calls").is_err());
        assert!(
            parse_nonstream(
                Protocol::OpenaiResponses,
                &json!({"status":"completed","output":[{"type":"function_call"}]})
            )
            .is_err()
        );
        assert!(parse_nonstream(Protocol::OpenaiChatCompletions,&json!({"choices":[{"message":{"content":"no","refusal":"no"},"finish_reason":"stop"}]})).is_err());
        let mut anthropic = Parser::new(Protocol::AnthropicMessages);
        anthropic.event(r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hidden"}}"#).unwrap();
        anthropic
            .event(
                r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"visible"}}"#,
            )
            .unwrap();
        anthropic
            .event(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        anthropic.event(r#"{"type":"message_stop"}"#).unwrap();
        assert_eq!(anthropic.finish().unwrap().text, "visible");
    }
    #[test]
    fn multiline_sse_comments() {
        let mut parser = Parser::new(Protocol::OpenaiChatCompletions);
        parser.line(":keepalive").unwrap();
        parser.line(r#"data: {"choices":[{"delta":"#).unwrap();
        parser
            .line(r#"data: {"content":"é"},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert_eq!(parser.line("").unwrap(), vec!["é"]);
        parser.line("data: [DONE]").unwrap();
        parser.line("").unwrap();
        assert_eq!(parser.finish().unwrap().text, "é");
    }
}
