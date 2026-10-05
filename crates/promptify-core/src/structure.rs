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
    #[error("the prompt has no loop; add a correction and repeat-verification loop such as \"Loop: if Step 2 fails the stated requirements, return to Step 1 to correct the mismatches; then recheck Step 2 (max 2 rounds).\"")]
    NoLoop,
    #[error("the prompt has no checks; add a non-empty \"Done when:\" section listing conditions that can fail, such as \"the tests pass\" or \"every claim has a source\"")]
    NoChecks,
    #[error("the prompt needs at least two steps: do the work, then verify it")]
    TooFewSteps,
    #[error("Step {step} has an empty body; describe the work or verification it performs")]
    EmptyStep { step: usize },
    #[error("Step {step} has malformed dependencies; use (after 1, 2) for prerequisites and add ; parallel with N only for an independent step. Use existing step numbers and never list the same step as both dependent and parallel")]
    DependencySyntax { step: usize },
    #[error("Step {step} repeats reference {on} in a dependency or parallel clause")]
    DuplicateReference { step: usize, on: usize },
    #[error("the graph has no dependency edges; make the verification step depend explicitly on the work it checks")]
    NoDependencies,
    #[error("Step {step} lists nonexistent or self parallel reference {on}")]
    ParallelReference { step: usize, on: usize },
    #[error("Step {step} cannot run parallel with Step {on}: one depends directly or transitively on the other")]
    ParallelConflict { step: usize, on: usize },
    #[error("use a meaningful bounded correction loop: \"Loop: if Step 2 fails the stated checks, return to Step 1 to correct the failed requirements; then recheck Step 2 (max 2 rounds).\" Name the failed checks, correction action, and verification step")]
    LoopCorrectionSyntax,
    #[error("loop verification Step {check} must be the failed-check step and depend directly or transitively on correction Step {target}; do not return to the checker itself")]
    LoopDoesNotRecheckWork { target: usize, check: usize },
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;.:])[ \t]*(?:[-*>#]+[ \t]*)?(?:\d+[.)][ \t]+)?(?:\*\*)?step[ \t]+(\d+)[ \t]*(?:\(([^)\n]*)\))?(?:[ \t]*\*\*)?[ \t]*[:\-–—]")
        .expect("valid heading regex")
});
static LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bloop(?:\*\*)?[ \t]*:").expect("valid loop regex"));
static STEP_REFS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsteps?[ \t]+(\d+(?:[ \t]*(?:-|–|,|and|to|or)[ \t]*\d+)*)").expect("valid step reference regex")
});
static INLINE_DEPENDENCY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(\s*(?:after|depends on)\s+\d+[^)\n]*\)").expect("valid inline dependency regex"));
static ROUNDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:max(?:imum)?|at most|up to|no more than)\.?[ \t]+(?:of[ \t]+)?(\d+)[ \t]*(?:rounds?|iterations?|attempts?|times|passes|cycles)\b")
        .expect("valid rounds regex")
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid number regex"));

fn numbers(text: &str) -> impl Iterator<Item = usize> + '_ {
    NUMBER.find_iter(text).map(|m| m.as_str().parse().unwrap_or(usize::MAX))
}

fn section_end(text: &str) -> usize {
    [HEADING.find(text), LOOP.find(text), DONE_WHEN.find(text)]
        .into_iter().flatten().map(|m| m.start()).min().unwrap_or(text.len())
}

fn nonempty(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

fn loop_body(text: &str) -> &str {
    let rest = text.trim_start_matches(|c: char| c.is_whitespace() || c == '*');
    &rest[..section_end(rest)]
}

/// Removes workflow metadata, but retains step references in the goal and task prose:
/// those can refer to a user-supplied document rather than this graph.
pub(crate) fn without_graph_metadata(text: &str) -> String {
    let mut spans: Vec<_> = HEADING.find_iter(text).map(|heading| (heading.start(), heading.end())).collect();
    for heading in LOOP.find_iter(text) {
        let rest = &text[heading.end()..];
        let segment = loop_body(rest);
        let start = heading.end() + rest.len() - rest.trim_start_matches(|c: char| c.is_whitespace() || c == '*').len();
        spans.extend(STEP_REFS.find_iter(segment).chain(ROUNDS.find_iter(segment)).map(|field| (start + field.start(), start + field.end())));
    }
    spans.sort_unstable();
    let mut result = String::new();
    let mut cursor = 0;
    for (start, end) in spans {
        if start >= cursor {
            result.push_str(&text[cursor..start]);
            result.push(' ');
        }
        cursor = cursor.max(end);
    }
    result.push_str(&text[cursor..]);
    result
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
        let segment = loop_body(&text[found.end()..]);

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

/// Final-output contract, stronger than the backwards-compatible structure inspection API.
/// Checks explicit graph edges and correction/recheck wiring, not the semantic truth of prose.
pub fn validate_graph(text: &str) -> Result<GraphSummary, GraphError> {
    if opens_with_role(text) {
        return Err(GraphError::RoleOpener);
    }
    let headings: Vec<_> = HEADING.captures_iter(text).collect();
    if let Some(marker) = STEP_START.find_iter(text).find(|marker| {
        !headings.iter().any(|heading| heading.get(0).unwrap().start() == marker.start())
    }) {
        let step = numbers(marker.as_str()).next().unwrap_or(1);
        return Err(GraphError::DependencySyntax { step });
    }
    let summary = validate_structure(text)?;
    if summary.steps == 0 {
        return Err(GraphError::NoSteps);
    }
    if summary.steps < 2 {
        return Err(GraphError::TooFewSteps);
    }
    let mut dependencies = vec![Vec::new(); summary.steps];
    let mut parallels = vec![Vec::new(); summary.steps];
    for (index, heading) in headings.iter().enumerate() {
        let step = index + 1;
        let heading_end = heading.get(0).unwrap().end();
        let body_end = headings.get(index + 1).and_then(|next| next.get(0)).map_or(text.len(), |next| next.start());
        let rest = &text[heading_end..body_end];
        if INLINE_DEPENDENCY.is_match(rest) {
            return Err(GraphError::DependencySyntax { step });
        }
        if !nonempty(&rest[..section_end(rest)]) {
            return Err(GraphError::EmptyStep { step });
        }
        if let Some(clause) = heading.get(2) {
            let caps = DEPENDENCIES.captures(clause.as_str().trim())
                .ok_or(GraphError::DependencySyntax { step })?;
            for capture in 1..=3 {
                let list = if capture == 1 { &mut dependencies[index] } else { &mut parallels[index] };
                if let Some(values) = caps.get(capture) {
                    for on in numbers(values.as_str()) {
                        if list.contains(&on) {
                            return Err(GraphError::DuplicateReference { step, on });
                        }
                        list.push(on);
                    }
                }
            }
        }
        for &on in &dependencies[index] {
            if on == 0 || on >= step {
                return Err(GraphError::ForwardDependency { step, on });
            }
        }
        for &on in &parallels[index] {
            if on == 0 || on > summary.steps || on == step {
                return Err(GraphError::ParallelReference { step, on });
            }
        }
    }
    if dependencies.iter().all(Vec::is_empty) {
        return Err(GraphError::NoDependencies);
    }
    // Dependencies always point backwards, so closure is computed in one ordered pass.
    let mut ancestors = vec![vec![false; summary.steps]; summary.steps];
    for index in 0..summary.steps {
        for &on in &dependencies[index] {
            ancestors[index][on - 1] = true;
            for prior in 0..on - 1 {
                ancestors[index][prior] |= ancestors[on - 1][prior];
            }
        }
    }
    for (index, parallel) in parallels.iter().enumerate() {
        for &on in parallel {
            if ancestors[index][on - 1] || ancestors[on - 1][index] {
                return Err(GraphError::ParallelConflict { step: index + 1, on });
            }
        }
    }
    if summary.loops == 0 {
        return Err(GraphError::NoLoop);
    }
    for found in LOOP.find_iter(text) {
        let segment = loop_body(&text[found.end()..]);
        let caps = CORRECTION_LOOP.captures(segment).ok_or(GraphError::LoopCorrectionSyntax)?;
        if !nonempty(&caps[2]) || !nonempty(&caps[4]) {
            return Err(GraphError::LoopCorrectionSyntax);
        }
        let failed = caps[1].parse::<usize>().unwrap_or(usize::MAX);
        let target = caps[3].parse::<usize>().unwrap_or(usize::MAX);
        let check = caps[5].parse::<usize>().unwrap_or(usize::MAX);
        for target in [failed, target, check] {
            if target == 0 || target > summary.steps {
                return Err(GraphError::LoopTargetMissing { target });
            }
        }
        if failed != check || !ancestors[check - 1][target - 1] {
            return Err(GraphError::LoopDoesNotRecheckWork { target, check });
        }
    }
    let has_checks = DONE_WHEN.find_iter(text).any(|heading| {
        let rest = &text[heading.end()..];
        nonempty(&rest[..section_end(rest)])
    });
    if !has_checks {
        return Err(GraphError::NoChecks);
    }
    Ok(summary)
}

static DEPENDENCIES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:after[ \t]+(\d+(?:[ \t]*,[ \t]*\d+)*)(?:[ \t]*;[ \t]*parallel with[ \t]+(\d+(?:[ \t]*,[ \t]*\d+)*))?|parallel with[ \t]+(\d+(?:[ \t]*,[ \t]*\d+)*))$").unwrap()
});

static STEP_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;.])[ \t]*(?:[-*>#]+[ \t]*)?(?:\d+[.)][ \t]+)?(?:\*\*)?step[ \t]+\d+").unwrap()
});

static CORRECTION_LOOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^if\s+Step\s+(\d+)\s+fails\s+([^;]+?),\s*return to Step\s+(\d+)\s+to\s+([^;]+?);\s*then recheck Step\s+(\d+)\s*\(max\s+(\d+)\s+rounds?\)\s*[.]?(?:\s|$)").unwrap()
});

static DONE_WHEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:^|[;.])[ \t]*(?:[-*>#]+[ \t]*)?(?:\*\*)?done when(?:\*\*)?[ \t]*:(?:[ \t]*\*\*)?")
        .expect("valid done-when regex")
});

#[cfg(test)]
mod tests {
    use super::*;

    const COMPACT: &str = "Explain the result.\nStep 1: Draft the explanation.\nStep 2 (after 1): Verify its accuracy against the supplied evidence.\nLoop: if Step 2 fails the accuracy checks, return to Step 1 to correct unsupported claims; then recheck Step 2 (max 2 rounds).\nDone when: the explanation is supported by the supplied evidence.";

    #[test]
    fn accepts_compact_multiline_inline_and_markdown_correction_graphs() {
        assert_eq!(validate_graph(COMPACT), Ok(GraphSummary { steps: 2, loops: 1 }));
        assert!(validate_graph(&COMPACT.replace('\n', "; ")).is_ok());
        let labeled = COMPACT.replace("Explain the result.\nStep 1:", "Explain the result; Task graph: Step 1:").replace('\n', "; ");
        assert!(validate_graph(&labeled).is_ok(), "a section label before Step 1 is not a missing step");
        assert!(validate_graph(&COMPACT.replace("the explanation is supported by the supplied evidence.", "Step 1 is supported by the supplied evidence.")).is_ok());
        let markdown = COMPACT.replace("Step 1:", "**Step 1:**")
            .replace("Step 2 (after 1):", "**Step 2 (after 1):**")
            .replace("Loop: if", "- **Loop:**\nIf")
            .replace("; then recheck", ";\nthen recheck")
            .replace("Done when:", "## **Done when:**\n");
        assert!(validate_graph(&markdown).is_ok(), "{markdown}");
    }

    #[test]
    fn rejects_one_step_empty_steps_and_decorative_graphs() {
        let one = "Step 1: Write it.\nLoop: if it fails, return to Step 1 (max 2 rounds).\nDone when: it is clear.";
        assert!(validate_structure(one).is_ok(), "legacy inspection remains available");
        assert_eq!(validate_graph(one), Err(GraphError::TooFewSteps));
        for body in ["", "...", "** **"] {
            assert_eq!(validate_graph(&COMPACT.replace("Draft the explanation.", body)), Err(GraphError::EmptyStep { step: 1 }));
            assert_eq!(validate_graph(&COMPACT.replace("Verify its accuracy against the supplied evidence.", body)), Err(GraphError::EmptyStep { step: 2 }));
        }
        assert_eq!(validate_graph(&COMPACT.replace(" (after 1)", "")), Err(GraphError::NoDependencies));
    }

    #[test]
    fn rejects_malformed_and_duplicate_dependencies() {
        for clause in ["after", "after one", "after 1,", "after 1 or loop exit", "depends on Step 1", "after 1; anything 1"] {
            let text = COMPACT.replace("after 1", clause);
            assert_eq!(validate_graph(&text), Err(GraphError::DependencySyntax { step: 2 }), "{clause}");
        }
        assert_eq!(validate_graph(&COMPACT.replace("after 1", "after 1, 1")), Err(GraphError::DuplicateReference { step: 2, on: 1 }));
        assert_eq!(validate_graph(&COMPACT.replace("(after 1):", "(after 1:")), Err(GraphError::DependencySyntax { step: 2 }));
        for on in [0, 2, 3] {
            assert_eq!(validate_graph(&COMPACT.replace("after 1", &format!("after {on}"))), Err(GraphError::ForwardDependency { step: 2, on }));
        }
    }

    #[test]
    fn accepts_independent_forward_parallel_work_and_join() {
        let graph = "Build a comparison.\nStep 1: Identify criteria.\nStep 2 (after 1; parallel with 3): Research option A.\nStep 3 (after 1; parallel with 2): Research option B.\nStep 4 (after 2, 3): Compare the options.\nStep 5 (after 4): Verify source support and consistent criteria.\nLoop: if Step 5 fails the source or consistency checks, return to Step 2 to repair the research and comparison; then recheck Step 5 (max 8 rounds).\nDone when: both options have supported findings under consistent criteria.";
        assert_eq!(validate_graph(graph), Ok(GraphSummary { steps: 5, loops: 1 }));
        assert!(validate_graph(&graph.replace('\n', "; ")).is_ok());
        assert_eq!(validate_graph(&graph.replace("parallel with 3", "parallel with 6")), Err(GraphError::ParallelReference { step: 2, on: 6 }));
        assert_eq!(validate_graph(&graph.replace("parallel with 3", "parallel with 2")), Err(GraphError::ParallelReference { step: 2, on: 2 }));
        assert_eq!(validate_graph(&graph.replace("parallel with 3", "parallel with 3, 3")), Err(GraphError::DuplicateReference { step: 2, on: 3 }));
        assert_eq!(validate_graph(&graph.replace("parallel with 3", "parallel with 4")), Err(GraphError::ParallelConflict { step: 2, on: 4 }));
        assert_eq!(validate_graph(&graph.replace("parallel with 3", "parallel with 5")), Err(GraphError::ParallelConflict { step: 2, on: 5 }));
        assert_eq!(validate_graph(&graph.replace("after 1; parallel with 2", "after 2; parallel with 2")), Err(GraphError::ParallelConflict { step: 2, on: 3 }));
    }

    #[test]
    fn loop_requires_explicit_failure_correction_recheck_and_bounds() {
        for bad in [
            "Loop: if the result is wrong, return to Step 1 (max 2 rounds).",
            "Loop: return to Step 1 to correct claims; then recheck Step 2 (max 2 rounds).",
            "Loop: if Step 2 fails ..., return to Step 1 to correct claims; then recheck Step 2 (max 2 rounds).",
            "Loop: if Step 2 fails accuracy, return to Step 1 to ...; then recheck Step 2 (max 2 rounds).",
            "Loop: if Step 2 fails accuracy, return to Step 1 to correct claims (max 2 rounds).",
        ] {
            let text = COMPACT.lines().map(|line| if line.starts_with("Loop:") { bad } else { line }).collect::<Vec<_>>().join("\n");
            assert_eq!(validate_graph(&text), Err(GraphError::LoopCorrectionSyntax), "{bad}");
        }
        assert_eq!(validate_graph(&COMPACT.replace("return to Step 1", "return to Step 2")), Err(GraphError::LoopDoesNotRecheckWork { target: 2, check: 2 }));
        assert_eq!(validate_graph(&COMPACT.replace("recheck Step 2", "recheck Step 1")), Err(GraphError::LoopDoesNotRecheckWork { target: 1, check: 1 }));
        for (from, to) in [("return to Step 1", "return to Step 3"), ("recheck Step 2", "recheck Step 3"), ("if Step 2", "if Step 3")] {
            assert_eq!(validate_graph(&COMPACT.replace(from, to)), Err(GraphError::LoopTargetMissing { target: 3 }));
        }
        assert_eq!(validate_graph(&COMPACT.replace("(max 2 rounds)", "")), Err(GraphError::LoopWithoutLimit));
        for rounds in [0, 9] {
            assert_eq!(validate_graph(&COMPACT.replace("max 2 rounds", &format!("max {rounds} rounds"))), Err(GraphError::LoopLimitOutOfRange { rounds }));
        }
        let unrelated = COMPACT.replace("\nLoop:", "\nStep 3: Verify unrelated work.\nLoop:")
            .replace("if Step 2", "if Step 3").replace("recheck Step 2", "recheck Step 3");
        assert_eq!(validate_graph(&unrelated), Err(GraphError::LoopDoesNotRecheckWork { target: 1, check: 3 }));
    }

    const GOOD: &str = "\
Fix the flaky upload test.

Step 1: Reproduce the failure and capture the error.
Step 2 (after 1): Find the root cause.
Step 3 (after 2): Fix it.
Step 4 (after 2; parallel with 3): Add a regression test.
Step 5 (after 3, 4): Run the full test suite.
Loop: if Step 5 fails the regression or suite checks, return to Step 3 to correct the implementation and affected tests; then recheck Step 5 (max 3 rounds).
Done when: the upload regression test and suite pass.";

    #[test]
    fn accepts_well_formed_graphs() {
        assert_eq!(validate_structure(GOOD), Ok(GraphSummary { steps: 5, loops: 1 }));
        let markdown = "**Step 1:** Plan.\n**Step 2 (after 1):** Build.\n- **Loop:** if review fails, repeat Steps 1-2, up to 2 rounds.";
        assert_eq!(validate_structure(markdown), Ok(GraphSummary { steps: 2, loops: 1 }));
        let inline = "Fix the build. Step 1: find the cause; Step 2 (after 1): fix it; Step 3 (after 2): run the tests; Loop: if tests fail, return to Step 2 (max 3 rounds); Done when: all tests pass.";
        assert_eq!(validate_structure(inline), Ok(GraphSummary { steps: 3, loops: 1 }));
    }

    #[test]
    fn a_reference_to_done_when_inside_a_loop_does_not_end_it() {
        let graph = "Research the evidence.\nStep 1: Find reliable sources.\nStep 2 (after 1): Check the findings.\nLoop: if Step 2 fails the criteria in Done when, return to Step 1 to replace unsupported evidence; then recheck Step 2 (max 3 rounds).\nDone when: every claim has a reliable source.";
        assert!(validate_graph(graph).is_ok());
        assert!(validate_graph(&graph.replace('\n', "; ")).is_ok());
        let missing = graph.replace("return to Step 1 to replace unsupported evidence;", "revise the findings;");
        assert_eq!(validate_graph(&missing), Err(GraphError::LoopCorrectionSyntax));
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
    fn rejects_dependency_metadata_outside_step_headings() {
        let malformed = GOOD.replace("Step 1: Reproduce the failure and capture the error.", "Step 1: Reproduce the failure and capture the error. (after 0)");
        assert_eq!(validate_graph(&malformed), Err(GraphError::DependencySyntax { step: 1 }));
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
            let text = format!("{base}{heading}If Step 2 fails the test checks,\nreturn to Step 1 to fix the failures;\nthen recheck Step 2 (max 3 rounds).\nDone when: all tests pass.");
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
        let graph = "Step 1: Build.\nStep 2 (after 1): Test.\nLoop: if Step 2 fails the tests, return to Step 1 to fix failures; then recheck Step 2 (max 2 rounds).";
        assert_eq!(validate_graph(graph), Err(GraphError::NoChecks));
        assert!(validate_graph(&format!("**Done when:** tests pass.\n{graph}")).is_ok(), "checks may come first");
        assert_eq!(validate_graph("Step 1: a\nStep 2 (after 2): b\nLoop: if it fails, return to Step 1 (max 2 rounds).\nDone when: b."), Err(GraphError::ForwardDependency { step: 2, on: 2 }));
    }

    #[test]
    fn completion_checks_must_have_their_own_nonempty_section() {
        let graph = "Step 1: Implement the change.\nStep 2 (after 1): Run the tests.\nLoop: if Step 2 fails the tests, return to Step 1 to fix the failures; then recheck Step 2 (max 2 rounds).";
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
        let inline = format!("Done when: all tests pass; {}", graph.replace('\n', "; "));
        assert!(validate_graph(&inline).is_ok());
        let empty_inline = format!("Done when: ; {}", graph.replace('\n', "; "));
        assert_eq!(validate_graph(&empty_inline), Err(GraphError::NoChecks));
    }
}
