use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::context::{ActiveContext, normalize_process};
use crate::structure::Structure;

pub const FALLBACK_PROFILE_ID: &str = "generic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    AiChat,
    CodeAgent,
    Search,
    ImageGen,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteChord {
    #[default]
    Standard,
    /// Ctrl+Shift+V on Windows/Linux terminals.
    Terminal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewlinePolicy {
    #[default]
    Keep,
    /// Join into one line so a shell cannot execute pasted lines one by one.
    Collapse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub said: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    processes: Vec<String>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    id: String,
    name: String,
    kind: ProfileKind,
    style: String,
    #[serde(default)]
    paste: PasteChord,
    #[serde(default)]
    newlines: NewlinePolicy,
    structure: Option<Structure>,
    #[serde(default)]
    examples: Vec<Example>,
    #[serde(default, rename = "match")]
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(rename = "profile")]
    profiles: Vec<RawProfile>,
}

#[derive(Debug, Clone)]
struct Rule {
    hosts: Vec<String>,
    processes: Vec<String>,
    title: Option<Regex>,
}

impl Rule {
    /// A known site (4) outranks process + title (3), which outranks process (2) or title (1).
    fn specificity(&self) -> usize {
        4 * usize::from(!self.hosts.is_empty()) + 2 * usize::from(!self.processes.is_empty()) + usize::from(self.title.is_some())
    }

    fn matches(&self, host: Option<&str>, process: &str, title: &str) -> bool {
        if !self.hosts.is_empty() {
            match host {
                Some(h) if self.hosts.iter().any(|p| host_matches(h, p)) => {}
                _ => return false,
            }
        }
        if !self.processes.is_empty() && !self.processes.iter().any(|p| p == process) {
            return false;
        }
        self.title.as_ref().is_none_or(|re| re.is_match(title))
    }
}

fn host_matches(host: &str, pattern: &str) -> bool {
    host == pattern || host.strip_suffix(pattern).is_some_and(|prefix| prefix.ends_with('.'))
}

#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub kind: ProfileKind,
    pub style: String,
    pub paste: PasteChord,
    pub newlines: NewlinePolicy,
    pub structure: Structure,
    pub examples: Vec<Example>,
    #[serde(skip)]
    rules: Vec<Rule>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid profile TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("profile {0:?}: {1}")]
    Invalid(String, String),
}

#[derive(Debug, Clone)]
pub struct ProfileSet {
    profiles: Vec<Profile>,
    fallback: usize,
}

const BUNDLED: &str = include_str!("../profiles/default.toml");

impl ProfileSet {
    pub fn bundled() -> Self {
        Self::from_toml(BUNDLED).expect("bundled profiles are validated by tests")
    }

    pub fn from_toml(source: &str) -> Result<Self, ProfileError> {
        let raw: RawFile = toml::from_str(source)?;
        let mut profiles = Vec::with_capacity(raw.profiles.len());
        for p in raw.profiles {
            let invalid = |msg: &str| ProfileError::Invalid(p.id.clone(), msg.to_owned());
            if p.id.trim().is_empty() || !p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(invalid("id must be non-empty [A-Za-z0-9_]"));
            }
            if profiles.iter().any(|existing: &Profile| existing.id == p.id) {
                return Err(invalid("duplicate id"));
            }
            if p.style.trim().is_empty() {
                return Err(invalid("style must not be empty"));
            }
            let structure = p.structure.unwrap_or(match (p.kind, p.newlines) {
                (ProfileKind::Search | ProfileKind::ImageGen, _) => Structure::Flat,
                (_, NewlinePolicy::Collapse) => Structure::Inline,
                _ => Structure::Graph,
            });
            if structure == Structure::Graph && p.newlines == NewlinePolicy::Collapse {
                return Err(invalid("graph structure needs line breaks; use inline"));
            }
            let mut rules = Vec::with_capacity(p.rules.len());
            for r in &p.rules {
                let title = match &r.title {
                    Some(pattern) => Some(
                        RegexBuilder::new(pattern)
                            .case_insensitive(true)
                            .size_limit(1 << 20)
                            .build()
                            .map_err(|e| invalid(&format!("bad title regex: {e}")))?,
                    ),
                    None => None,
                };
                let rule = Rule {
                    hosts: r.hosts.iter().map(|h| h.trim().to_ascii_lowercase()).collect(),
                    processes: r.processes.iter().map(|n| normalize_process(n)).collect(),
                    title,
                };
                if rule.specificity() == 0 {
                    return Err(invalid("match rule has no conditions"));
                }
                rules.push(rule);
            }
            profiles.push(Profile {
                id: p.id,
                name: p.name,
                kind: p.kind,
                style: p.style.trim().to_owned(),
                paste: p.paste,
                newlines: p.newlines,
                structure,
                examples: p.examples,
                rules,
            });
        }
        let fallback = profiles
            .iter()
            .position(|p| p.id == FALLBACK_PROFILE_ID)
            .ok_or_else(|| ProfileError::Invalid(FALLBACK_PROFILE_ID.into(), "fallback profile missing".into()))?;
        Ok(Self { profiles, fallback })
    }

    /// Most specific matching rule wins; ties go to the profile listed first.
    pub fn resolve(&self, ctx: &ActiveContext) -> &Profile {
        let host = ctx.url_host();
        let process = ctx.normalized_process();
        let mut best: Option<(usize, usize)> = None;
        for (index, profile) in self.profiles.iter().enumerate() {
            for rule in &profile.rules {
                if !rule.matches(host.as_deref(), &process, &ctx.window_title) {
                    continue;
                }
                let score = rule.specificity();
                if best.is_none_or(|(_, s)| score > s) {
                    best = Some((index, score));
                }
            }
        }
        &self.profiles[best.map_or(self.fallback, |(index, _)| index)]
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn all(&self) -> &[Profile] {
        &self.profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(process: &str, title: &str, url: Option<&str>) -> ActiveContext {
        ActiveContext {
            process_name: process.into(),
            window_title: title.into(),
            url: url.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn bundled_profiles_parse() {
        let set = ProfileSet::bundled();
        assert!(set.all().len() >= 8);
        assert!(set.get(FALLBACK_PROFILE_ID).is_some());
        assert!(set.all().iter().all(|p| p.examples.iter().all(|e| !e.said.is_empty() && !e.prompt.is_empty())));
    }

    #[test]
    fn resolves_sites_apps_and_terminals() {
        let set = ProfileSet::bundled();
        let id = |c: ActiveContext| set.resolve(&c).id.clone();
        assert_eq!(id(ctx("chrome.exe", "ChatGPT", Some("https://chatgpt.com/c/abc"))), "chatgpt");
        assert_eq!(id(ctx("firefox", "Claude", Some("claude.ai/new"))), "claude");
        assert_eq!(id(ctx("Claude.exe", "Claude", None)), "claude");
        assert_eq!(id(ctx("Cursor.exe", "main.rs - app - Cursor", None)), "cursor");
        assert_eq!(id(ctx("chrome.exe", "Grok", Some("https://grok.com/c/1"))), "grok");
        assert_eq!(id(ctx("chrome.exe", "Grok / X", Some("https://x.com/i/grok"))), "grok");
        assert_eq!(id(ctx("chrome.exe", "Home / X", Some("https://x.com/home"))), FALLBACK_PROFILE_ID);
        assert_eq!(id(ctx("zen.exe", "Grok — Zen Browser", None)), "grok");
        assert_eq!(id(ctx("zen.exe", "Trip ideas - Claude — Zen Browser", None)), "claude");
        assert_eq!(id(ctx("zen.exe", "Weather — Zen Browser", None)), FALLBACK_PROFILE_ID);
        assert_eq!(id(ctx("chrome.exe", "Claude vs Grok", Some("https://chatgpt.com/c/1"))), "chatgpt", "site outranks title");
        assert_eq!(id(ctx("WindowsTerminal.exe", "✳ Claude Code", None)), "claude_code");
        assert_eq!(id(ctx("WindowsTerminal.exe", "pwsh in repo", None)), "terminal");
        assert_eq!(id(ctx("chrome.exe", "Inbox", Some("https://mail.google.com"))), FALLBACK_PROFILE_ID);
        assert_eq!(id(ctx("notepad.exe", "Untitled", None)), FALLBACK_PROFILE_ID);
    }

    #[test]
    fn host_suffix_requires_label_boundary() {
        assert!(host_matches("chat.openai.com", "openai.com"));
        assert!(host_matches("claude.ai", "claude.ai"));
        assert!(!host_matches("notclaude.ai", "claude.ai"));
        assert!(!host_matches("claude.ai.evil.com", "claude.ai"));
    }

    #[test]
    fn more_specific_rule_wins_over_order() {
        let set = ProfileSet::from_toml(
            r#"
            [[profile]]
            id = "generic"
            name = "Generic"
            kind = "ai_chat"
            style = "g"
            [[profile.match]]
            processes = ["term"]
            [[profile]]
            id = "agent"
            name = "Agent"
            kind = "code_agent"
            style = "a"
            [[profile.match]]
            processes = ["term"]
            title = "agent"
            "#,
        )
        .unwrap();
        assert_eq!(set.resolve(&ctx("term", "my AGENT", None)).id, "agent");
        assert_eq!(set.resolve(&ctx("term", "shell", None)).id, "generic");
    }

    #[test]
    fn rejects_invalid_profiles() {
        let base = |extra: &str| format!(
            "[[profile]]\nid = \"generic\"\nname = \"G\"\nkind = \"ai_chat\"\nstyle = \"s\"\n{extra}"
        );
        assert!(ProfileSet::from_toml(&base("[[profile.match]]\n")).is_err());
        assert!(ProfileSet::from_toml(&base("[[profile.match]]\ntitle = \"(\"\n")).is_err());
        assert!(ProfileSet::from_toml(&base("unknown = 1\n")).is_err());
        assert!(ProfileSet::from_toml(&base("").replace("generic", "other")).is_err());
        let dup = format!("{}\n{}", base(""), base(""));
        assert!(ProfileSet::from_toml(&dup).is_err());
    }
}
