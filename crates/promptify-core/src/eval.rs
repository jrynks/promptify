//! Offline evaluation of prompt structure against recorded requests.

use serde::Deserialize;
use crate::context::ActiveContext;
use crate::profiles::ProfileSet;
use crate::routing::{self, PromptForm, Rendering, RoutingOptions, Surface, TaskId};

use crate::structure::{validate_graph, validate_structure};
pub use crate::structure::opens_with_role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    Graph,
    Flat,
}

/// Case IDs are opaque so nothing descriptive can leak into a run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub id: String,
    pub said: String,
    pub expect: Expect,
    #[serde(default = "default_process")]
    pub process: String,
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
}

fn default_process() -> String {
    "chrome.exe".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvalFile {
    #[serde(rename = "case")]
    cases: Vec<EvalCase>,
}

pub fn load_cases(source: &str) -> Result<Vec<EvalCase>, String> {
    let file: EvalFile = toml::from_str(source).map_err(|e| e.to_string())?;
    if file.cases.is_empty() {
        return Err("evaluation file must contain at least one case".into());
    }
    let mut seen = std::collections::HashSet::new();
    for case in &file.cases {
        if !seen.insert(case.id.as_str()) {
            return Err(format!("duplicate case id {}", case.id));
        }
    }
    Ok(file.cases)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveCase {
    pub id: String,
    pub said: String,
    #[serde(default = "default_process")]
    pub process: String,
    #[serde(default = "default_chat_url")]
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
    pub task_type: Option<TaskId>,
    pub form: Option<PromptForm>,
    pub surface: Option<Surface>,
    #[serde(default)]
    pub expect_error: bool,
    #[serde(default)]
    pub required: Vec<String>,
}

fn default_chat_url() -> Option<String> {
    Some("https://chatgpt.com".into())
}

impl AdaptiveCase {
    pub fn context(&self) -> ActiveContext {
        ActiveContext { process_name: self.process.clone(), window_title: self.title.clone(), url: self.url.clone(), ..Default::default() }
    }

    pub fn options(&self) -> RoutingOptions {
        RoutingOptions { rendering: Rendering::Adaptive, surface: self.surface, ..Default::default() }
    }
}

pub fn load_adaptive_cases(source: &str) -> Result<Vec<AdaptiveCase>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        case: Vec<AdaptiveCase>,
    }
    let file: File = toml::from_str(source).map_err(|e| e.to_string())?;
    if file.case.is_empty() {
        return Err("adaptive evaluation needs at least one case".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for case in &file.case {
        if case.id.trim().is_empty() || !ids.insert(case.id.clone()) || case.said.trim().is_empty() {
            return Err("adaptive cases require unique IDs and non-empty requests".into());
        }
        if case.expect_error == (case.task_type.is_some() || case.form.is_some())
            || (!case.expect_error && (case.task_type.is_none() || case.form.is_none()))
        {
            return Err(format!("{}: specify task_type and form, or expect_error", case.id));
        }
        if let Some(id) = &case.task_type {
            if id.as_str() != "general.request" && routing::catalog().get(id.as_str()).is_none() {
                return Err(format!("{}: unknown expected task", case.id));
            }
            for required in &case.required {
                regex::RegexBuilder::new(required).case_insensitive(true).size_limit(1 << 20).build()
                    .map_err(|error| format!("{}: invalid required-detail pattern: {error}", case.id))?;
            }
        }
    }
    Ok(file.case)
}

pub fn score_adaptive_output(case: &AdaptiveCase, policy: &routing::ResolvedPromptPolicy, text: &str) -> Result<(), String> {
    routing::validate_rewrite(policy, &case.said, text)?;
    for required in &case.required {
        let pattern = regex::RegexBuilder::new(required).case_insensitive(true).size_limit(1 << 20).build().map_err(|e| e.to_string())?;
        if !pattern.is_match(text) {
            return Err(format!("{}: a required user detail is missing from the generated prompt", case.id));
        }
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct RoutingScore {
    pub total: usize,
    pub passed: usize,
    pub macro_f1: f64,
    pub failures: Vec<String>,
}

pub fn score_routing(cases: &[AdaptiveCase]) -> RoutingScore {
    let profiles = ProfileSet::bundled();
    let mut labels = std::collections::BTreeMap::<String, (usize, usize, usize)>::new();
    let mut predictions = Vec::new();
    let mut failures = Vec::new();
    for case in cases {
        let context = case.context();
        let result = routing::resolve(&context, profiles.resolve(&context), &case.said, &case.options());
        let pass = match &result {
            Ok(policy) => !case.expect_error && case.task_type.as_ref() == Some(&policy.task_type) && case.form == Some(policy.form),
            Err(_) => case.expect_error,
        };
        if !pass { failures.push(case.id.clone()); }
        if let Some(id) = &case.task_type { labels.entry(id.as_str().into()).or_default(); }
        predictions.push((case.task_type.as_ref(), result.ok().map(|policy| policy.task_type)));
    }
    for (label, (tp, fp, missing)) in &mut labels {
        for (expected, actual) in &predictions {
            let expected = expected.is_some_and(|id| id.as_str() == label);
            let actual = actual.as_ref().is_some_and(|id| id.as_str() == label);
            match (expected, actual) {
                (true, true) => *tp += 1,
                (false, true) => *fp += 1,
                (true, false) => *missing += 1,
                _ => {}
            }
        }
    }
    let macro_f1 = if labels.is_empty() { 0.0 } else {
        labels.values().map(|(tp, fp, missing)| {
            let denominator = 2 * tp + fp + missing;
            if denominator == 0 { 0.0 } else { (2 * tp) as f64 / denominator as f64 }
        }).sum::<f64>() / labels.len() as f64
    };
    RoutingScore { total: cases.len(), passed: cases.len() - failures.len(), macro_f1, failures }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseScore {
    pub valid: bool,
    pub structured: bool,
    pub pass: bool,
}

pub fn score(expect: Expect, output: Option<&str>) -> CaseScore {
    let Some(output) = output.filter(|text| !text.trim().is_empty()) else {
        return CaseScore { valid: false, structured: false, pass: false };
    };
    match validate_structure(output) {
        Ok(summary) => {
            let structured = summary.is_structured();
            let valid = expect != Expect::Graph || validate_graph(output).is_ok();
            let pass = valid && structured == (expect == Expect::Graph);
            CaseScore { valid, structured, pass }
        }
        Err(_) => CaseScore { valid: false, structured: true, pass: false },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_regression_benchmark_covers_enabled_types_and_target_contracts() {
        let cases = load_adaptive_cases(include_str!("../../../eval/adaptive.toml")).unwrap();
        for task in routing::catalog().all().iter().filter(|task| task.status == routing::RecipeStatus::Enabled) {
            assert!(cases.iter().any(|case| case.task_type.as_ref() == Some(&task.id)), "{}", task.id.as_str());
        }
        let score = score_routing(&cases);
        assert!(score.macro_f1 >= 0.90, "{score:?}");
        assert_eq!(score.passed, score.total, "{score:?}");
    }

    #[test]
    fn bundled_cases_parse_with_opaque_ids() {
        let cases = load_cases(include_str!("../../../eval/cases.toml")).unwrap();
        assert!(cases.len() >= 20);
        assert!(cases.iter().all(|c| c.expect == Expect::Graph), "all final prompts now require graphs");
        assert!(cases.iter().all(|c| c.id.len() <= 4 && c.id.starts_with('c')));
    }

    #[test]
    fn intent_expansion_cases_include_live_request_and_route_correctly() {
        let cases = load_adaptive_cases(include_str!("../../../eval/intent-expansion.toml")).unwrap();
        assert_eq!(cases.iter().filter(|case|
            case.said == "Can you think of any other ways to improve prompt quality"
        ).count(), 3);
        let score = score_routing(&cases);
        assert_eq!(score.passed, score.total, "{score:?}");
    }

    #[test]
    fn scoring_requires_valid_structure_matching_expectation() {
        let graph = "Step 1: a\nStep 2 (after 1): b\nLoop: if b fails, return to Step 1 (max 2 rounds).\nDone when: b passes.";
        assert!(score(Expect::Graph, Some(graph)).pass);
        let persona = format!("Act as a senior DevOps engineer.\n{graph}");
        assert_eq!(score(Expect::Graph, Some(&persona)), CaseScore { valid: false, structured: true, pass: false });
        assert!(!score(Expect::Graph, Some("Step 1: a\nStep 2 (after 1): b")).pass, "a graph needs a loop");
        assert!(!score(Expect::Flat, Some(graph)).pass);
        assert!(score(Expect::Flat, Some("Just a prompt.")).pass);
        assert!(!score(Expect::Graph, Some("Just a prompt.")).pass);
        let broken = "Step 1: a\nStep 2 (after 2): b";
        assert_eq!(score(Expect::Graph, Some(broken)), CaseScore { valid: false, structured: true, pass: false });
        assert!(!score(Expect::Flat, None).pass);
        assert!(!score(Expect::Flat, Some(" \n ")).pass);
        for checks in ["", "\nDone when:", "\nDone when: ..."] {
            let incomplete = format!("Step 1: a\nLoop: if a fails, return to Step 1 (max 2 rounds).{checks}");
            assert_eq!(score(Expect::Graph, Some(&incomplete)), CaseScore { valid: false, structured: true, pass: false });
        }
        let dup = "[[case]]\nid = \"c01\"\nsaid = \"x\"\nexpect = \"flat\"\n[[case]]\nid = \"c01\"\nsaid = \"y\"\nexpect = \"flat\"\n";
        assert!(load_cases(dup).is_err());
        assert!(load_cases("case = []").is_err());
    }

    #[test]
    fn role_openers_are_detected() {
        for role in [
            "Act as a pricing strategist. I need...", "**Act as** an expert", "## __Act as__ an expert",
            "> `Act as` an expert", "You are an expert travel planner.", "  you're a senior engineer",
            "As a helpful assistant, explain...", "Act \n as a senior engineer.", "\"Act as a developer.",
            "You\u{2019}re an expert.", "Assume the role of a reviewer.", "Adopt the role of an editor.",
        ] {
            assert!(opens_with_role(role), "{role}");
        }
        for goal in ["I need to choose a pricing model.", "Help me plan a party.", "Review the draft as a skeptical auditor in Step 2.", "Fix the login button."] {
            assert!(!opens_with_role(goal), "{goal}");
        }
    }
}
