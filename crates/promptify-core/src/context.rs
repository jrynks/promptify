use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Identifies the exact window the user was in when the hotkey was pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowIdentity {
    pub handle: u64,
    pub process_id: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActiveContext {
    pub window: WindowIdentity,
    pub process_name: String,
    pub window_title: String,
    pub url: Option<String>,
}

impl ActiveContext {
    pub fn normalized_process(&self) -> String {
        normalize_process(&self.process_name)
    }

    pub fn url_host(&self) -> Option<String> {
        let raw = self.url.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        let parsed = match url::Url::parse(raw) {
            Ok(parsed) if parsed.host_str().is_some() => parsed,
            // Browser address bars usually omit the scheme.
            _ if !raw.contains("://") => url::Url::parse(&format!("https://{raw}")).ok()?,
            _ => return None,
        };
        let host = parsed.host_str()?.to_ascii_lowercase();
        Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
    }

    /// Key used for per-app settings: the site for browsers, otherwise the process.
    pub fn app_key(&self) -> String {
        self.url_host().unwrap_or_else(|| self.normalized_process())
    }
}

pub fn normalize_process(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// Text read from the focused element. Backends must set `is_secure` for password fields.
#[derive(Debug, Clone)]
pub struct FocusedText {
    pub text: String,
    pub is_secure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedText {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPolicy {
    /// App keys (see [`ActiveContext::app_key`]) the user opted in for surrounding-text capture.
    pub surrounding_text_apps: BTreeSet<String>,
    pub max_surrounding_chars: usize,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self { surrounding_text_apps: BTreeSet::new(), max_surrounding_chars: 4000 }
    }
}

impl ContextPolicy {
    pub fn allows_surrounding_text(&self, ctx: &ActiveContext) -> bool {
        self.max_surrounding_chars > 0 && self.surrounding_text_apps.contains(&ctx.app_key())
    }

    pub fn admit(&self, ctx: &ActiveContext, focused: FocusedText) -> Option<AdmittedText> {
        if focused.is_secure || !self.allows_surrounding_text(ctx) {
            return None;
        }
        let text = focused.text.trim();
        if text.is_empty() {
            return None;
        }
        let total = text.chars().count();
        if total <= self.max_surrounding_chars {
            return Some(AdmittedText { text: text.to_owned(), truncated: false });
        }
        // Keep the tail: text nearest the cursor is usually the most relevant.
        let tail: String = text.chars().skip(total - self.max_surrounding_chars).collect();
        Some(AdmittedText { text: tail, truncated: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(process: &str, url: Option<&str>) -> ActiveContext {
        ActiveContext {
            process_name: process.into(),
            url: url.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn normalizes_process_paths_and_extensions() {
        assert_eq!(normalize_process(r"C:\Program Files\Google\Chrome\chrome.EXE"), "chrome");
        assert_eq!(normalize_process("/usr/bin/gnome-terminal-server"), "gnome-terminal-server");
        assert_eq!(normalize_process("Google Chrome"), "google chrome");
    }

    #[test]
    fn extracts_host_with_or_without_scheme() {
        assert_eq!(ctx("chrome", Some("https://www.chatgpt.com/c/1")).url_host().as_deref(), Some("chatgpt.com"));
        assert_eq!(ctx("chrome", Some("claude.ai/new")).url_host().as_deref(), Some("claude.ai"));
        assert_eq!(ctx("chrome", Some("localhost:3000")).url_host().as_deref(), Some("localhost"));
        assert_eq!(ctx("chrome", Some("about:blank")).url_host(), None);
        assert_eq!(ctx("chrome", Some("  ")).url_host(), None);
    }

    #[test]
    fn app_key_prefers_host() {
        assert_eq!(ctx("chrome.exe", Some("https://claude.ai")).app_key(), "claude.ai");
        assert_eq!(ctx("Cursor.exe", None).app_key(), "cursor");
    }

    fn policy(apps: &[&str], max: usize) -> ContextPolicy {
        ContextPolicy {
            surrounding_text_apps: apps.iter().map(|s| s.to_string()).collect(),
            max_surrounding_chars: max,
        }
    }

    #[test]
    fn admits_only_opted_in_non_secure_text() {
        let c = ctx("chrome", Some("https://mail.google.com"));
        let text = || FocusedText { text: "thread".into(), is_secure: false };
        assert_eq!(policy(&[], 100).admit(&c, text()), None);
        assert_eq!(policy(&["mail.google.com"], 0).admit(&c, text()), None);
        assert_eq!(
            policy(&["mail.google.com"], 100).admit(&c, FocusedText { text: "hunter2".into(), is_secure: true }),
            None
        );
        assert_eq!(
            policy(&["mail.google.com"], 100).admit(&c, text()),
            Some(AdmittedText { text: "thread".into(), truncated: false })
        );
    }

    #[test]
    fn truncation_keeps_tail_and_reports_it() {
        let c = ctx("notes", None);
        let admitted = policy(&["notes"], 3)
            .admit(&c, FocusedText { text: "abcdéf".into(), is_secure: false })
            .unwrap();
        assert_eq!(admitted, AdmittedText { text: "déf".into(), truncated: true });
    }
}
