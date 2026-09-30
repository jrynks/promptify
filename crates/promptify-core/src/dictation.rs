use std::sync::LazyLock;

use regex::Regex;

static HESITATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i:u+m+|u+h+m*|e+r+m*|h+m+)$").unwrap());

static NON_SPEECH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[^\]]*\]|\((?i:music|silence|applause|laughter|laughs|noise|inaudible|blank audio|sound|beep|coughs?|sighs?)[^)]*\)|\*[^*]*\*").unwrap()
});

/// Removes speech-model annotations such as `[BLANK_AUDIO]`, `(music)` or `*coughs*`.
pub fn strip_non_speech(text: &str) -> String {
    NON_SPEECH.replace_all(text, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

const TERMINAL: [char; 3] = ['.', '!', '?'];

/// Removes spoken fillers while keeping the speaker's words. Deliberately conservative:
/// "like" and "you know" are only removed when set off as a parenthetical.
pub fn remove_fillers(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut capitalize_next = false;
    let mut i = 0;
    while i < tokens.len() {
        let at_start = out.last().is_none_or(|t| t.ends_with(TERMINAL));
        let prev_comma = out.last().is_some_and(|t| t.ends_with(','));
        let prev_conjunction = out.last().is_some_and(|t| matches!(t.to_lowercase().as_str(), "and" | "but" | "so" | "or"));
        let (core, trail) = split_trailing(tokens[i]);
        let parenthetical = at_start || prev_comma || prev_conjunction;

        let hesitation = HESITATION.is_match(core);
        let parenthetical_like = core.eq_ignore_ascii_case("like") && trail.starts_with(',') && parenthetical;
        let span = if hesitation || parenthetical_like {
            1
        } else if core.eq_ignore_ascii_case("you")
            && parenthetical
            && tokens.get(i + 1).is_some_and(|next| {
                let (c, t) = split_trailing(next);
                c.eq_ignore_ascii_case("know") && t.starts_with(',')
            })
        {
            2
        } else {
            0
        };

        if span == 0 {
            let token = if capitalize_next { capitalize(tokens[i]) } else { tokens[i].to_owned() };
            capitalize_next = false;
            out.push(token);
            i += 1;
            continue;
        }

        let (_, last_trail) = split_trailing(tokens[i + span - 1]);
        if let Some(end) = last_trail.chars().find(|c| TERMINAL.contains(c)) {
            if let Some(prev) = out.last_mut() {
                if prev.ends_with(',') {
                    prev.pop();
                }
                if !prev.ends_with(TERMINAL) {
                    prev.push(end);
                }
            }
            capitalize_next = true;
        } else if prev_comma && last_trail.starts_with(',') {
            if let Some(prev) = out.last_mut() {
                prev.pop();
            }
        } else if at_start {
            capitalize_next = true;
        }
        i += span;
    }
    out.join(" ")
}

fn split_trailing(token: &str) -> (&str, &str) {
    let core = token.trim_end_matches(|c: char| c.is_ascii_punctuation());
    (core, &token[core.len()..])
}

fn capitalize(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_non_speech_annotations() {
        assert_eq!(strip_non_speech(" [BLANK_AUDIO]"), "");
        assert_eq!(strip_non_speech("(music) Draft the email. *coughs* Thanks"), "Draft the email. Thanks");
        assert_eq!(strip_non_speech("Call it (maybe) tomorrow"), "Call it (maybe) tomorrow");
    }

    #[test]
    fn removes_hesitations() {
        assert_eq!(remove_fillers("Um, we're locking the beta for March 3rd."), "We're locking the beta for March 3rd.");
        assert_eq!(remove_fillers("and uh I'll draft the announcement"), "and I'll draft the announcement");
        assert_eq!(remove_fillers("I'll draft, uh, the announcement"), "I'll draft the announcement");
        assert_eq!(remove_fillers("that is the plan, umm."), "that is the plan.");
        assert_eq!(remove_fillers("Hmm. okay then"), "Okay then");
    }

    #[test]
    fn removes_parenthetical_like_and_you_know() {
        assert_eq!(remove_fillers("the billing, like, migration"), "the billing migration");
        assert_eq!(remove_fillers("and you know, the billing migration"), "and the billing migration");
        assert_eq!(remove_fillers("You know, it works."), "It works.");
    }

    #[test]
    fn keeps_meaningful_words() {
        let kept = [
            "I like the proposal.",
            "Do you know the answer?",
            "It looks like, honestly, fine.",
            "5 mm bolts and an umbrella",
            "Ask Sara if you know, she knows.",
        ];
        for text in kept {
            assert_eq!(remove_fillers(text), text);
        }
    }
}
