use crate::profiles::NewlinePolicy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    pub text: String,
    /// True when the output exceeded `max_chars` and was cut; callers must not insert it silently.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SanitizeError {
    #[error("the model produced no usable text")]
    Empty,
}

pub fn sanitize_output(raw: &str, newlines: NewlinePolicy, max_chars: usize) -> Result<Sanitized, SanitizeError> {
    let mut text = strip_think_blocks(raw);
    text = text.replace("\r\n", "\n").trim().to_owned();
    text = strip_preamble(&text);
    text = strip_wrapping_fence(&text);
    text = strip_wrapping_quotes(&text);
    if newlines == NewlinePolicy::Collapse {
        text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    // A trailing newline would submit the prompt in many chat boxes and shells.
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(SanitizeError::Empty);
    }
    let total = text.chars().count();
    if total > max_chars {
        return Ok(Sanitized { text: text.chars().take(max_chars).collect(), truncated: true });
    }
    Ok(Sanitized { text, truncated: false })
}

fn strip_think_blocks(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    loop {
        match rest.find("<think>") {
            Some(start) => {
                out.push_str(&rest[..start]);
                match rest[start..].find("</think>") {
                    Some(end) => rest = &rest[start + end + "</think>".len()..],
                    // An unterminated reasoning block has no usable answer after it.
                    None => return out,
                }
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

fn strip_preamble(text: &str) -> String {
    let Some((first, rest)) = text.split_once('\n') else {
        return text.to_owned();
    };
    let lower = first.trim().to_ascii_lowercase();
    let starts = ["here is", "here's", "sure", "certainly", "okay", "ok,", "below is", "rewritten prompt", "prompt:", "refined prompt"];
    if lower.ends_with(':') && lower.len() <= 80 && starts.iter().any(|s| lower.starts_with(s)) {
        rest.trim_start().to_owned()
    } else {
        text.to_owned()
    }
}

fn strip_wrapping_fence(text: &str) -> String {
    if let Some(inner) = text.strip_prefix("```").and_then(|t| t.strip_suffix("```")) {
        let body = inner.split_once('\n').map_or("", |(_lang, body)| body);
        if !body.contains("```") {
            return body.trim().to_owned();
        }
    }
    text.to_owned()
}

fn strip_wrapping_quotes(text: &str) -> String {
    for (open, close) in [('"', '"'), ('“', '”')] {
        if let Some(inner) = text.strip_prefix(open).and_then(|t| t.strip_suffix(close))
            && !inner.contains(open)
            && !inner.contains(close)
        {
            return inner.trim().to_owned();
        }
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(raw: &str) -> String {
        sanitize_output(raw, NewlinePolicy::Keep, 10_000).unwrap().text
    }

    #[test]
    fn strips_reasoning_preamble_fence_and_quotes() {
        assert_eq!(clean("<think>plan it</think>\nWrite a haiku."), "Write a haiku.");
        assert_eq!(clean("Here's your refined prompt:\n\nWrite a haiku.\n"), "Write a haiku.");
        assert_eq!(clean("```markdown\nWrite a haiku.\n- short\n```"), "Write a haiku.\n- short");
        assert_eq!(clean("\"Write a haiku.\""), "Write a haiku.");
    }

    #[test]
    fn keeps_legitimate_content() {
        assert_eq!(clean("Summarize this: the report below.\nKeep it short."), "Summarize this: the report below.\nKeep it short.");
        assert_eq!(clean("Explain \"ownership\" and \"borrowing\" in Rust."), "Explain \"ownership\" and \"borrowing\" in Rust.");
        assert_eq!(clean("Fix it:\n```rs\nfn a() {}\n```\nthen test"), "Fix it:\n```rs\nfn a() {}\n```\nthen test");
    }

    #[test]
    fn unterminated_think_is_empty() {
        assert_eq!(sanitize_output("<think>still going", NewlinePolicy::Keep, 100), Err(SanitizeError::Empty));
        assert_eq!(sanitize_output("  \n ", NewlinePolicy::Keep, 100), Err(SanitizeError::Empty));
    }

    #[test]
    fn collapse_joins_lines_and_never_ends_with_newline() {
        let out = sanitize_output("Do this\n- and that\r\n\n", NewlinePolicy::Collapse, 100).unwrap();
        assert_eq!(out.text, "Do this - and that");
        assert!(!clean("line\n\n").ends_with('\n'));
    }

    #[test]
    fn reports_truncation() {
        let out = sanitize_output("abcdef", NewlinePolicy::Keep, 3).unwrap();
        assert_eq!(out, Sanitized { text: "abc".into(), truncated: true });
    }
}
