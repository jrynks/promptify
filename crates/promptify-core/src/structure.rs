//! Task-graph prompt structure: request-shape hint and output validation.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const MAX_STEPS: usize = 12;
pub const MAX_LOOP_ROUNDS: usize = 8;

/// How a profile's prompts may be structured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Structure {
    /// Numbered steps and loops, one per line.
    Graph,
    /// Steps and loops written inline in a single paragraph.
    Inline,
    /// Never add steps or loops (image and video generation).
    Flat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    SingleTask,
    MultiStep,
}

/// How involved a request is. Sets the size of the task graph and how many clarifying questions the
/// prompt may ask: none, at most 1, or up to 3, and only when details that matter are missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    Simple,
    Moderate,
    Complex,
}

impl Complexity {
    pub fn max_questions(self) -> usize {
        match self {
            Self::Simple => 0,
            Self::Moderate => 1,
            Self::Complex => 3,
        }
    }
}

/// Personal plans and decisions usually hinge on details the speaker did not give (budget, dates, who).
const DECISION_STEMS: &[&str] =
    &["plan", "buy", "purchas", "choos", "pick", "recommend", "decid", "compar", "budget", "trip", "travel", "vacation", "party", "hire", "invest", "strateg", "shortlist", "should"];

fn words(transcript: &str) -> Vec<String> {
    transcript.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Multi-step work, or a longer request to plan, buy or decide something, is complex; other longer
/// requests are moderate; short ones are simple.
pub fn complexity(transcript: &str) -> Complexity {
    if complexity_hint(transcript) == Shape::MultiStep {
        return Complexity::Complex;
    }
    let words = words(transcript);
    let decision = words.iter().any(|w| DECISION_STEMS.iter().any(|stem| w.starts_with(stem) && w.len() <= stem.len() + 4));
    match words.len() {
        n if n >= 12 && decision => Complexity::Complex,
        n if n >= 12 => Complexity::Moderate,
        _ => Complexity::Simple,
    }
}

const TASK_VERBS: &[&str] = &[
    "plan", "research", "compar", "build", "writ", "draft", "design", "implement", "refactor", "analy", "creat", "migrat",
    "fix", "debug", "test", "review", "evaluat", "organiz", "organis", "prepar", "outlin", "summar", "shortlist", "pick",
    "choos", "decid", "recommend", "add", "updat", "deploy", "check", "verif", "find", "schedul", "interview", "synthes",
    "estimat", "investigat", "translat", "rewrit", "edit", "refin", "benchmark", "profil", "measur", "optimi",
];
const SEQUENCE_WORDS: &[&str] =
    &["then", "after that", "afterwards", "next", "finally", "first", "once", "before", "followed by", "step", "steps"];
const ITERATION_WORDS: &[&str] = &[
    "until", "till", "iterate", "iterating", "repeat", "again", "loop", "make sure", "double check", "verify", "keep going",
    "keep trying",
];

/// Cheap heuristic so a small model does not have to judge request complexity itself.
pub fn complexity_hint(transcript: &str) -> Shape {
    let words = words(transcript);
    if words.len() < 8 {
        return Shape::SingleTask;
    }
    let verbs = TASK_VERBS
        .iter()
        .filter(|stem| {
            let slack = if stem.len() <= 3 { 3 } else { 4 };
            words.iter().any(|w| w.starts_with(*stem) && w.len() <= stem.len() + slack)
        })
        .count();
    let joined = format!(" {} ", words.join(" "));
    let has = |list: &[&str]| list.iter().filter(|p| joined.contains(&format!(" {p} "))).count();
    let cues = has(SEQUENCE_WORDS) + has(ITERATION_WORDS);
    let iteration = has(ITERATION_WORDS);
    if verbs >= 3 || (verbs >= 2 && cues >= 1) || (verbs >= 1 && iteration >= 1 && words.len() >= 15) {
        Shape::MultiStep
    } else {
        Shape::SingleTask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphSummary {
    pub steps: usize,
    pub loops: usize,
}

impl GraphSummary {
    pub fn is_structured(&self) -> bool {
        self.steps > 0 || self.loops > 0
    }
}

/// Detects persona boilerplate at the opening, not a perspective used inside a task step.
pub fn opens_with_role(prompt: &str) -> bool {
    let plain = prompt.replace(['*', '_', '`'], "").replace('\u{2019}', "'");
    let first = plain
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '#' | '-' | '>' | '"' | '\'' | '\u{201C}' | '\u{201D}'))
        .split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    [
        "act as ", "you are a ", "you are an ", "as an expert", "as a ", "imagine you are", "pretend you are",
        "you're a ", "you're an ", "assume the role of ", "adopt the role of ",
    ].iter().any(|prefix| first.starts_with(prefix))
}

/// Content-free description of an invalid task-graph prompt; also used as the repair instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("the prompt opens with a role/persona instruction such as \"Act as...\"; remove that opener and start with the user's goal instead, keeping any useful perspective inside a task step")]
    RoleOpener,
    #[error("the prompt has {0} steps; use at most {MAX_STEPS}")]
    TooManySteps(usize),
    #[error("the step at position {position} is numbered {found}; number the steps 1, 2, 3 in order with no gaps or repeats")]
    Numbering { position: usize, found: usize },
    #[error("Step {step} depends on step {on}, but a step may depend only on earlier steps")]
    ForwardDependency { step: usize, on: usize },
    #[error("a loop has no numbered return target; include the literal words \"return to Step N\" in that loop, replacing N with an existing step number; \"revise it\" or \"rewrite that point\" alone is not a return target")]
    LoopWithoutTarget,
    #[error("a loop returns to step {target}, which does not exist")]
    LoopTargetMissing { target: usize },
    #[error("a loop has no round limit")]
    LoopWithoutLimit,
    #[error("a loop allows {rounds} rounds; use between 1 and {MAX_LOOP_ROUNDS}")]
    LoopLimitOutOfRange { rounds: usize },
    #[error("the prompt has no numbered steps; lay the work out as Step 1, Step 2 (after 1) and so on")]
    NoSteps,
    #[error("the prompt has no loop; add a check loop such as \"Loop: if the result misses a requirement, return to Step 1 (max 2 rounds).\"")]
    NoLoop,
    #[error("the prompt has no checks; add a non-empty \"Done when:\" section listing conditions that can fail, such as \"the tests pass\" or \"every claim has a source\"")]
    NoChecks,
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;.])[ \t]*(?:[-*>#]+[ \t]*)?(?:\d+[.)][ \t]+)?(?:\*\*)?step[ \t]+(\d+)[ \t]*(?:\(([^)\n]*)\))?(?:[ \t]*\*\*)?[ \t]*[:\-–—]")
        .expect("valid heading regex")
});
static LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bloop(?:\*\*)?[ \t]*:").expect("valid loop regex"));
static LOOP_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bdone when\b|\bloop(?:\*\*)?[ \t]*:").expect("valid loop end regex"));
static STEP_REFS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsteps?[ \t]+(\d+(?:[ \t]*(?:-|–|,|and|to|or)[ \t]*\d+)*)").expect("valid step reference regex")
});
static ROUNDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:max(?:imum)?|at most|up to|no more than)\.?[ \t]+(?:of[ \t]+)?(\d+)[ \t]*(?:rounds?|iterations?|attempts?|times|passes|cycles)\b")
        .expect("valid rounds regex")
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid number regex"));

fn numbers(text: &str) -> impl Iterator<Item = usize> + '_ {
    NUMBER.find_iter(text).map(|m| m.as_str().parse().unwrap_or(usize::MAX))
}

/// Checks numbered steps, dependency order (which rules out cycles) and loop bounds.
/// Text without any steps or loops is valid and reports an empty summary.
pub fn validate_structure(text: &str) -> Result<GraphSummary, GraphError> {
    let headings: Vec<_> = HEADING.captures_iter(text).collect();
    if headings.len() > MAX_STEPS {
        return Err(GraphError::TooManySteps(headings.len()));
    }
    for (index, caps) in headings.iter().enumerate() {
        let position = index + 1;
        let found = caps[1].parse().unwrap_or(usize::MAX);
        if found != position {
            return Err(GraphError::Numbering { position, found });
        }
        if let Some(clause) = caps.get(2).map(|m| m.as_str().trim()) {
            let lower = clause.to_ascii_lowercase();
            let deps = ["after", "depends on"].iter().find_map(|kw| lower.strip_prefix(kw));
            if let Some(deps) = deps {
                let deps = deps.split(';').next().unwrap_or_default();
                if let Some(on) = numbers(deps).find(|&on| on == 0 || on >= position) {
                    return Err(GraphError::ForwardDependency { step: position, on });
                }
            }
        }
    }

    let steps = headings.len();
    let mut loops = 0;
    for found in LOOP.find_iter(text) {
        loops += 1;
        let rest = text[found.end()..].trim_start_matches(|c: char| c.is_whitespace() || c == '*');
        let mut end = rest.find('\n').unwrap_or(rest.len());
        let line = &rest[..end];
        for cut in [LOOP_END.find(line).map(|m| m.start()), HEADING.find(line).map(|m| m.start())].into_iter().flatten() {
            end = end.min(cut);
        }
        let segment = &rest[..end];

        let targets: Vec<usize> = STEP_REFS.captures_iter(segment).flat_map(|c| numbers(c.get(1).unwrap().as_str()).collect::<Vec<_>>()).collect();
        if targets.is_empty() {
            return Err(GraphError::LoopWithoutTarget);
        }
        if let Some(&target) = targets.iter().find(|&&t| t == 0 || t > steps) {
            return Err(GraphError::LoopTargetMissing { target });
        }
        let rounds: Vec<usize> = ROUNDS.captures_iter(segment).map(|c| c[1].parse().unwrap_or(usize::MAX)).collect();
        if rounds.is_empty() {
            return Err(GraphError::LoopWithoutLimit);
        }
        if let Some(&rounds) = rounds.iter().find(|&&r| r == 0 || r > MAX_LOOP_ROUNDS) {
            return Err(GraphError::LoopLimitOutOfRange { rounds });
        }
    }
    Ok(GraphSummary { steps, loops })
}

/// A prompt for an AI must be a well-formed loop or graph: numbered steps, at least one loop, and a
/// "Done when" line with the checks the work must pass.
pub fn validate_graph(text: &str) -> Result<GraphSummary, GraphError> {
    if opens_with_role(text) {
        return Err(GraphError::RoleOpener);
    }
    let summary = validate_structure(text)?;
    if summary.steps == 0 {
        return Err(GraphError::NoSteps);
    }
    if summary.loops == 0 {
        return Err(GraphError::NoLoop);
    }
    let has_checks = DONE_WHEN.find_iter(text).any(|heading| {
        let rest = &text[heading.end()..];
        let end = [HEADING.find(rest), LOOP.find(rest), DONE_WHEN.find(rest)]
            .into_iter().flatten().map(|m| m.start()).min().unwrap_or(rest.len());
        rest[..end].chars().any(char::is_alphanumeric)
    });
    if !has_checks {
        return Err(GraphError::NoChecks);
    }
    Ok(summary)
}

static DONE_WHEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;.])[ \t]*(?:[-*>#]+[ \t]*)?(?:\*\*)?done when(?:\*\*)?[ \t]*:(?:[ \t]*\*\*)?")
        .expect("valid done-when regex")
});

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
Fix the flaky upload test.

Step 1: Reproduce the failure and capture the error.
Step 2 (after 1): Find the root cause.
Step 3 (after 2): Fix it.
Step 4 (after 2; parallel with 3): Add a regression test.
Step 5 (after 3, 4): Run the full test suite.
Loop: if any test fails, return to Step 2 (max 3 rounds).
Done when: the suite passes 5 times in a row.";

    #[test]
    fn accepts_well_formed_graphs() {
        assert_eq!(validate_structure(GOOD), Ok(GraphSummary { steps: 5, loops: 1 }));
        let markdown = "**Step 1:** Plan.\n**Step 2 (after 1):** Build.\n- **Loop:** if review fails, repeat Steps 1-2, up to 2 rounds.";
        assert_eq!(validate_structure(markdown), Ok(GraphSummary { steps: 2, loops: 1 }));
        let inline = "Fix the build. Step 1: find the cause; Step 2 (after 1): fix it; Step 3 (after 2): run the tests; Loop: if tests fail, return to Step 2 (max 3 rounds); Done when: all tests pass.";
        assert_eq!(validate_structure(inline), Ok(GraphSummary { steps: 3, loops: 1 }));
    }

    #[test]
    fn well_formed_graphs_still_reject_persona_openers() {
        for opener in ["Act as a senior DevOps engineer.", "**Act as** an expert.", "You are an experienced reviewer.", "Act\nas a senior engineer."] {
            let text = format!("{opener}\n{GOOD}");
            assert!(validate_structure(&text).is_ok(), "the shape alone is valid");
            assert_eq!(validate_graph(&text), Err(GraphError::RoleOpener));
        }
        let perspective = GOOD.replace("Fix it.", "Review and fix it as a senior engineer.");
        assert!(validate_graph(&perspective).is_ok(), "useful perspectives inside steps are allowed");
    }

    #[test]
    fn plain_prompts_are_valid_and_unstructured() {
        let plain = "Act as a travel planner. Plan a 3-day trip.\n\n1. Ask about budget.\n2. Suggest an itinerary.";
        let summary = validate_structure(plain).unwrap();
        assert!(!summary.is_structured());
        assert!(validate_structure("Go back to step 2 if needed.").is_ok_and(|s| !s.is_structured()));
    }

    #[test]
    fn rejects_bad_numbering() {
        let gap = "Step 1: a\nStep 3 (after 1): b";
        assert_eq!(validate_structure(gap), Err(GraphError::Numbering { position: 2, found: 3 }));
        let repeat = "Step 1: a\nStep 1: b";
        assert_eq!(validate_structure(repeat), Err(GraphError::Numbering { position: 2, found: 1 }));
        let huge = "Step 99999999999999999999999: a";
        assert!(matches!(validate_structure(huge), Err(GraphError::Numbering { position: 1, .. })));
    }

    #[test]
    fn rejects_forward_self_and_zero_dependencies() {
        assert_eq!(validate_structure("Step 1: a\nStep 2 (after 3): b\nStep 3: c"), Err(GraphError::ForwardDependency { step: 2, on: 3 }));
        assert_eq!(validate_structure("Step 1: a\nStep 2 (after 2): b"), Err(GraphError::ForwardDependency { step: 2, on: 2 }));
        assert_eq!(validate_structure("Step 1 (after 0): a"), Err(GraphError::ForwardDependency { step: 1, on: 0 }));
        assert_eq!(validate_structure("Step 1: a\nStep 2 (depends on Step 1 and 2): b"), Err(GraphError::ForwardDependency { step: 2, on: 2 }));
        assert!(validate_structure("Step 1: a\nStep 2 (parallel with 3): b\nStep 3: c").is_ok());
    }

    #[test]
    fn rejects_unbounded_or_dangling_loops() {
        let base = "Step 1: a\nStep 2 (after 1): b\n";
        assert_eq!(validate_structure(&format!("{base}Loop: if it fails, try again (max 2 rounds).")), Err(GraphError::LoopWithoutTarget));
        assert_eq!(validate_structure(&format!("{base}Loop: if it fails, return to Step 4 (max 2 rounds).")), Err(GraphError::LoopTargetMissing { target: 4 }));
        assert_eq!(validate_structure(&format!("{base}Loop: if it fails, return to Step 1 until it works.")), Err(GraphError::LoopWithoutLimit));
        assert_eq!(validate_structure(&format!("{base}Loop: if it fails, return to Step 1 (max 20 rounds).")), Err(GraphError::LoopLimitOutOfRange { rounds: 20 }));
        assert_eq!(validate_structure(&format!("{base}Loop: return to Step 1 (max 0 rounds).")), Err(GraphError::LoopLimitOutOfRange { rounds: 0 }));
        assert_eq!(validate_structure("Loop: return to Step 1 (max 2 rounds)."), Err(GraphError::LoopTargetMissing { target: 1 }));
        assert!(validate_structure(&format!("{base}Loop: if it fails, return to Step 1 (max {MAX_LOOP_ROUNDS} rounds).")).is_ok());
        let rounds = MAX_LOOP_ROUNDS + 1;
        assert_eq!(validate_structure(&format!("{base}Loop: if it fails, return to Step 1 (max {rounds} rounds).")), Err(GraphError::LoopLimitOutOfRange { rounds }));
    }

    #[test]
    fn loop_segment_stops_at_the_next_section() {
        let text = "Step 1: a; Loop: if it fails, return to Step 1; Done when: max 99 times is never reached";
        assert_eq!(validate_structure(text), Err(GraphError::LoopWithoutLimit));
    }

    #[test]
    fn loop_headings_may_be_on_their_own_line() {
        let base = "Step 1: Build.\nStep 2 (after 1): Test.\n";
        for heading in ["Loop:\n", "**Loop:**\n\n", "- **Loop:**\r\n"] {
            let text = format!("{base}{heading}If a test fails, return to Step 1 (max 3 rounds).\nDone when: all tests pass.");
            assert_eq!(validate_graph(&text), Ok(GraphSummary { steps: 2, loops: 1 }));
        }
        let missing = format!("{base}**Loop:**\n**Done when:** Step 1 succeeds within max 2 rounds.");
        assert_eq!(validate_structure(&missing), Err(GraphError::LoopWithoutTarget));
        let unbounded = format!("{base}**Loop:**\nIf a test fails, return to Step 1.\nDone when: max 2 rounds is never reached.");
        assert_eq!(validate_structure(&unbounded), Err(GraphError::LoopWithoutLimit));
    }

    #[test]
    fn caps_step_count() {
        let many: String = (1..=MAX_STEPS + 1).map(|n| format!("Step {n}: x\n")).collect();
        assert_eq!(validate_structure(&many), Err(GraphError::TooManySteps(MAX_STEPS + 1)));
        let max: String = (1..=MAX_STEPS).map(|n| format!("Step {n}: x\n")).collect();
        assert!(validate_structure(&max).is_ok());
    }

    #[test]
    fn complexity_hint_separates_simple_and_multi_step_requests() {
        let simple = [
            "what's the capital of france",
            "write a haiku about autumn leaves for my mom's birthday card",
            "um i want to buy a used car for my son he just got his license something safe and not too expensive",
            "so i need to plan a birthday party for my daughter she's turning eight and she loves science stuff",
            "translate this paragraph into formal german please",
        ];
        let multi = [
            "research the top three crm tools compare their pricing and then recommend one for a five person team",
            "the build is broken after the dependency update figure out what changed fix it and then run the tests until they pass",
            "draft a blog post about our launch review it for tone and keep editing until it's under 800 words",
            "the login button thing um when you click it twice it sends two requests fix that and add a test for it",
        ];
        for s in simple {
            assert_eq!(complexity_hint(s), Shape::SingleTask, "{s}");
        }
        for m in multi {
            assert_eq!(complexity_hint(m), Shape::MultiStep, "{m}");
        }
    }

    #[test]
    fn complexity_ramps_from_simple_to_complex() {
        let simple = ["what's the capital of france", "make me a picture of a fox in the snow", "translate this paragraph into formal german please"];
        let moderate = [
            "write a haiku about autumn leaves for my mom's birthday card please",
            "explain how a heat pump works in winter and why it is more efficient than a furnace",
        ];
        let complex = [
            "um i want to buy a used car for my son he just got his license something safe and not too expensive",
            "so i need to plan a birthday party for my daughter she's turning eight and she loves science stuff",
            "research the top three crm tools compare their pricing and then recommend one for a five person team",
        ];
        for (said, expected) in simple.iter().map(|s| (s, Complexity::Simple)).chain(moderate.iter().map(|s| (s, Complexity::Moderate))).chain(complex.iter().map(|s| (s, Complexity::Complex))) {
            assert_eq!(complexity(said), expected, "{said}");
        }
        assert_eq!([Complexity::Simple, Complexity::Moderate, Complexity::Complex].map(Complexity::max_questions), [0, 1, 3]);
    }

    #[test]
    fn graphs_need_steps_a_loop_and_checks() {
        assert_eq!(validate_graph(GOOD), Ok(GraphSummary { steps: 5, loops: 1 }));
        assert_eq!(validate_graph("Just a prompt."), Err(GraphError::NoSteps));
        assert_eq!(validate_graph("Step 1: a\nStep 2 (after 1): b\nDone when: b works."), Err(GraphError::NoLoop));
        assert_eq!(validate_graph("Step 1: a\nLoop: if a fails, return to Step 1 (max 2 rounds)."), Err(GraphError::NoChecks));
        assert!(validate_graph("**Done when:** a passes.\nStep 1: a\nLoop: if a fails, return to Step 1 (max 2 rounds).").is_ok(), "checks may come first");
        assert_eq!(validate_graph("Step 1: a\nStep 2 (after 2): b\nLoop: if it fails, return to Step 1 (max 2 rounds).\nDone when: b."), Err(GraphError::ForwardDependency { step: 2, on: 2 }));
    }

    #[test]
    fn completion_checks_must_have_their_own_nonempty_section() {
        let graph = "Step 1: Run the tests.\nLoop: if a test fails, return to Step 1 (max 2 rounds).";
        for empty in ["Done when:", "Done when: ...", "**Done when:**", "Done when: ;"] {
            for text in [format!("{empty}\n{graph}"), format!("{graph}\n{empty}")] {
                assert_eq!(validate_graph(&text), Err(GraphError::NoChecks), "{text}");
            }
        }
        let mention = format!("{graph}\nExplain what \"Done when:\" means.");
        assert_eq!(validate_graph(&mention), Err(GraphError::NoChecks));
        for checks in ["Done when: all tests pass.", "**Done when:**\n- All tests pass.", "## Done when:\nAll tests pass."] {
            assert!(validate_graph(&format!("{checks}\n{graph}")).is_ok(), "{checks}");
            assert!(validate_graph(&format!("{graph}\n{checks}")).is_ok(), "{checks}");
        }
        let inline = "Done when: all tests pass; Step 1: Run the tests; Loop: if a test fails, return to Step 1 (max 2 rounds).";
        assert!(validate_graph(inline).is_ok());
        let empty_inline = "Done when: ; Step 1: Run the tests; Loop: if a test fails, return to Step 1 (max 2 rounds).";
        assert_eq!(validate_graph(empty_inline), Err(GraphError::NoChecks));
    }
}
