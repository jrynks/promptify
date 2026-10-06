use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Custom,
    LmStudio,
    Openai,
    Anthropic,
    Google,
}
impl Provider {
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Custom => None,
            Self::LmStudio => Some("http://localhost:1234/v1"),
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
            Self::Google => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    None,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InferenceSelection {
    #[default]
    BundledLocal,
    Connection {
        connection_id: String,
        model: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceConnection {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub base_url: String,
    pub protocol: Protocol,
    pub auth: AuthMode,
    pub model: String,
    pub stream: bool,
    pub allow_insecure_lan: bool,
    pub consent_remote: bool,
    pub credential_present: bool,
    pub revision: u64,
}

// Write-only secrets must not acquire Serialize or Debug.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionInput {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub base_url: String,
    pub protocol: Protocol,
    pub auth: AuthMode,
    pub model: String,
    pub stream: bool,
    pub allow_insecure_lan: bool,
    pub consent_remote: bool,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub remove_secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceConfig {
    pub version: u32,
    pub revision: u64,
    pub selection: InferenceSelection,
    pub connections: Vec<InferenceConnection>,
}
impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            selection: Default::default(),
            connections: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Configured,
    Ready,
    Error,
}
pub type InferenceState = StatusState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceStatus {
    pub state: StatusState,
    pub selection: InferenceSelection,
    pub revision: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionGate {
    pub provider: &'static str,
    pub available: bool,
    pub reason: &'static str,
}
pub fn subscription_gates() -> Vec<SubscriptionGate> {
    ["chatgpt","supergrok"].into_iter().map(|provider|SubscriptionGate {
        provider,available:false,reason:"Subscription OAuth is not implemented. Use API keys, which are billed separately from subscriptions.",
    }).collect()
}
