use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

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

pub const MAX_VOCABULARY_WORDS: usize = 100;
pub const MAX_REPLACEMENTS: usize = 100;
pub const MAX_TERM_CHARS: usize = 60;
/// Whisper reads at most ~224 prompt tokens; this keeps the glossary well inside that.
pub const MAX_WHISPER_PROMPT_CHARS: usize = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

/// The user's own words: names and terms speech recognition should expect, and fixed corrections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vocabulary {
    pub words: Vec<String>,
    pub replacements: Vec<Replacement>,
}

fn clean_term(term: &str) -> Option<String> {
    let term: String = term.chars().filter(|c| !c.is_control()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    (!term.is_empty() && term.chars().count() <= MAX_TERM_CHARS).then_some(term)
}

impl Vocabulary {
    /// Trims, de-duplicates and caps user input, dropping anything unusable.
    pub fn sanitized(&self) -> Self {
        let mut words: Vec<String> = Vec::new();
        for word in self.words.iter().filter_map(|w| clean_term(w)) {
            if words.len() < MAX_VOCABULARY_WORDS && !words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                words.push(word);
            }
        }
        let mut replacements: Vec<Replacement> = Vec::new();
        for r in &self.replacements {
            let (Some(from), Some(to)) = (clean_term(&r.from), clean_term(&r.to).or_else(|| r.to.trim().is_empty().then(String::new))) else { continue };
            if replacements.len() < MAX_REPLACEMENTS && !replacements.iter().any(|x| x.from.eq_ignore_ascii_case(&from)) {
                replacements.push(Replacement { from, to });
            }
        }
        Self { words, replacements }
    }

    /// A short glossary for whisper's initial prompt, or `None` without words.
    pub fn whisper_prompt(&self) -> Option<String> {
        let mut prompt = String::from("Glossary:");
        let mut any = false;
        for word in &self.sanitized().words {
            if prompt.chars().count() + word.chars().count() + 3 > MAX_WHISPER_PROMPT_CHARS {
                break;
            }
            prompt.push_str(if any { ", " } else { " " });
            prompt.push_str(word);
            any = true;
        }
        any.then(|| prompt + ".")
    }

    /// Applies the replacements in one pass, whole words only and ignoring case, so a replacement's
    /// output is never replaced again.
    pub fn apply(&self, text: &str) -> String {
        let clean = self.sanitized();
        if clean.replacements.is_empty() {
            return text.to_owned();
        }
        let mut terms: Vec<&Replacement> = clean.replacements.iter().collect();
        terms.sort_by_key(|r| std::cmp::Reverse(r.from.chars().count()));
        let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let alternation: Vec<String> = terms
            .iter()
            .map(|r| {
                let start = if is_word(r.from.chars().next()) { r"\b" } else { "" };
                let end = if is_word(r.from.chars().last()) { r"\b" } else { "" };
                format!("{start}{}{end}", regex::escape(&r.from))
            })
            .collect();
        let Ok(pattern) = Regex::new(&format!("(?i)(?:{})", alternation.join("|"))) else { return text.to_owned() };
        let replaced = pattern.replace_all(text, |caps: &regex::Captures<'_>| {
            let found = &caps[0];
            terms.iter().find(|r| r.from.eq_ignore_ascii_case(found) || r.from.to_lowercase() == found.to_lowercase()).map_or_else(|| found.to_owned(), |r| r.to.clone())
        });
        replaced.split(' ').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spoken {
    NewLine,
    NewParagraph,
    ScratchThat,
}

const SPOKEN: [(&[&str], Spoken); 4] = [
    (&["new", "line"], Spoken::NewLine),
    (&["new", "paragraph"], Spoken::NewParagraph),
    (&["scratch", "that"], Spoken::ScratchThat),
    (&["delete", "that"], Spoken::ScratchThat),
];

const CLAUSE_END: [char; 6] = ['.', '!', '?', ',', ':', ';'];

/// Applies "new line", "new paragraph" and "scratch that"/"delete that" when said on their own,
/// as a separate sentence or clause. Inside a sentence ("delete that file") they stay as words.
pub fn apply_spoken_commands(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut scratched = false;
    let mut i = 0;
    'tokens: while i < tokens.len() {
        let starts_clause = i == 0 || tokens[i - 1].ends_with(CLAUSE_END);
        if starts_clause {
            for (words, command) in SPOKEN {
                let Some(candidate) = tokens.get(i..i + words.len()) else { continue };
                let matches = candidate.iter().zip(words).all(|(t, w)| split_trailing(t).0.eq_ignore_ascii_case(w));
                let last = candidate[words.len() - 1];
                let inner_clean = candidate[..words.len() - 1].iter().all(|t| split_trailing(t).1.is_empty());
                let ends_clause = i + words.len() == tokens.len() || last.ends_with(CLAUSE_END);
                if matches && inner_clean && ends_clause {
                    match command {
                        Spoken::NewLine => out.push("\n".into()),
                        Spoken::NewParagraph => out.push("\n\n".into()),
                        Spoken::ScratchThat => {
                            // Drop the sentence before the command, keeping earlier sentences and line breaks.
                            if out.last().is_some_and(|t| !t.starts_with('\n')) {
                                out.pop();
                                while out.last().is_some_and(|t| !t.starts_with('\n') && !t.ends_with(TERMINAL)) {
                                    out.pop();
                                }
                            }
                            scratched = true;
                        }
                    }
                    i += words.len();
                    continue 'tokens;
                }
            }
        }
        // Dictation can continue a sentence already on screen, so the first word keeps its case.
        let starts_sentence = match out.last() {
            Some(t) => t.starts_with('\n') || (scratched && t.ends_with(TERMINAL)),
            None => scratched,
        };
        scratched = false;
        out.push(if starts_sentence { capitalize(tokens[i]) } else { tokens[i].to_owned() });
        i += 1;
    }
    let mut joined = String::new();
    for token in &out {
        if token.starts_with('\n') {
            joined = joined.trim_end_matches([' ', ',']).to_owned();
            joined.push_str(token);
        } else {
            if !joined.is_empty() && !joined.ends_with('\n') {
                joined.push(' ');
            }
            joined.push_str(token);
        }
    }
    joined.trim().to_owned()
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

    fn vocab(words: &[&str], pairs: &[(&str, &str)]) -> Vocabulary {
        Vocabulary {
            words: words.iter().map(|w| w.to_string()).collect(),
            replacements: pairs.iter().map(|(f, t)| Replacement { from: f.to_string(), to: t.to_string() }).collect(),
        }
    }

    #[test]
    fn replacements_are_whole_word_case_insensitive_and_single_pass() {
        let v = vocab(&[], &[("prompt if I", "Promptify"), ("sea sharp", "C#"), ("C#", "loop"), ("jira", "Jira")]);
        assert_eq!(v.apply("open prompt if I and file a JIRA ticket"), "open Promptify and file a Jira ticket");
        assert_eq!(v.apply("learn sea sharp"), "learn C#", "output is never replaced again");
        assert_eq!(v.apply("jiras and ninjira"), "jiras and ninjira", "whole words only");
        assert_eq!(vocab(&[], &[("uh", "")]).apply("so uh yes"), "so yes");
    }

    #[test]
    fn vocabulary_is_sanitized_and_the_whisper_prompt_bounded() {
        let mut words: Vec<String> = (0..300).map(|i| format!("Term{i}")).collect();
        words.push("  Kubernetes \u{0007} ".into());
        words.push("x".repeat(MAX_TERM_CHARS + 1));
        words.push("term0".into());
        let v = Vocabulary { words, replacements: vec![] }.sanitized();
        assert_eq!(v.words.len(), MAX_VOCABULARY_WORDS);
        assert!(!v.words.iter().any(|w| w.chars().count() > MAX_TERM_CHARS));
        let prompt = vocab(&["Promptify", "Qwen", "Tauri"], &[]).whisper_prompt().unwrap();
        assert_eq!(prompt, "Glossary: Promptify, Qwen, Tauri.");
        let long = Vocabulary { words: (0..100).map(|i| format!("LongTermNumber{i}")).collect(), replacements: vec![] };
        assert!(long.whisper_prompt().unwrap().chars().count() <= MAX_WHISPER_PROMPT_CHARS);
        assert_eq!(Vocabulary::default().whisper_prompt(), None);
    }

    #[test]
    fn spoken_commands_apply_only_as_their_own_clause() {
        assert_eq!(apply_spoken_commands("Dear team. New line. Thanks for the update."), "Dear team.\nThanks for the update.");
        assert_eq!(apply_spoken_commands("First point. New paragraph. Second point."), "First point.\n\nSecond point.");
        assert_eq!(apply_spoken_commands("Send it Friday. Scratch that. Send it Monday."), "Send it Monday.");
        assert_eq!(apply_spoken_commands("Hello all. Send it Friday, scratch that, send it Monday."), "Hello all. Send it Monday.");
        assert_eq!(apply_spoken_commands("Keep this. Drop this. Delete that"), "Keep this.");
        assert_eq!(apply_spoken_commands("Line one. New line. Scratch that. Line two."), "Line one.\nLine two.", "never deletes across a line break");
        assert_eq!(apply_spoken_commands("Keep. New line. Half sentence, scratch that, done."), "Keep.\nDone.", "stops at the line break, not the earlier sentence");
    }

    #[test]
    fn command_words_inside_sentences_are_kept() {
        assert_eq!(apply_spoken_commands("and then we ship it"), "and then we ship it", "first word keeps its case");
        assert_eq!(apply_spoken_commands("Scratch that. send it"), "Send it");
        for text in [
            "Please delete that file before Friday.",
            "We need a new line of credit.",
            "Add a new paragraph about pricing.",
            "Scratch that itch.",
            "Don't scratch that, it will scar.",
        ] {
            assert_eq!(apply_spoken_commands(text), text);
        }
    }
}
