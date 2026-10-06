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
pub struct QualityPromptCase {
    pub id: String,
    pub category: String,
    pub said: String,
    #[serde(default = "default_process")]
    pub process: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: String,
    /// Explicit synthetic reference only; never loaded from the user's history.
    #[serde(default)]
    pub previous_prompt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityDictationCase {
    pub id: String,
    pub category: String,
    pub said: String,
}

#[derive(Debug, Clone)]
pub struct QualityEvalSuite {
    pub prompt: Vec<QualityPromptCase>,
    pub heldout_prompt: Vec<QualityPromptCase>,
    pub dictation: Vec<QualityDictationCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityEvalFile {
    #[serde(default, rename = "prompt")]
    prompt: Vec<QualityPromptCase>,
    #[serde(default, rename = "heldout_prompt")]
    heldout_prompt: Vec<QualityPromptCase>,
    #[serde(default)]
    dictation: Vec<QualityDictationCase>,
}

pub fn load_quality_suite(source: &str) -> Result<QualityEvalSuite, String> {
    let file: QualityEvalFile = toml::from_str(source).map_err(|error| error.to_string())?;
    if file.prompt.is_empty() || file.heldout_prompt.is_empty() || file.dictation.is_empty() {
        return Err("quality evaluation requires original prompt, held-out prompt, and dictation cases".into());
    }
    let mut seen = std::collections::HashSet::new();
    for id in file.prompt.iter().map(|case| &case.id)
        .chain(file.heldout_prompt.iter().map(|case| &case.id))
        .chain(file.dictation.iter().map(|case| &case.id))
    {
        if id.trim().is_empty() || !seen.insert(id.as_str()) {
            return Err(format!("quality evaluation has an empty or duplicate case id {id:?}"));
        }
    }
    Ok(QualityEvalSuite { prompt: file.prompt, heldout_prompt: file.heldout_prompt, dictation: file.dictation })
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
    fn saved_local_model_comparison_uses_the_production_contract() {
        let cases = load_cases(include_str!("../../../eval/graph-quality.toml")).unwrap();
        assert_eq!(cases.len(), 7);
        let evidence: serde_json::Value = serde_json::from_str(include_str!("../../../eval/graph-quality-results.json")).unwrap();
        for phase in ["baseline", "updated"] {
            let outputs = evidence[phase].as_array().unwrap();
            assert_eq!(outputs.len(), cases.len() * 2);
            let mut passed = 0;
            for output in outputs {
                let case = cases.iter().find(|case| output["case_id"].as_str() == Some(case.id.as_str())).unwrap();
                let text = output["text"].as_str();
                let result = score(case.expect, text);
                passed += usize::from(result.pass);
                if phase == "updated" && text.is_some() {
                    assert!(result.pass, "{}: {:?}", case.id, output["rendering"]);
                }
                if let Some(text) = text {
                    assert_eq!(text.chars().count() as u64, output["output_characters"].as_u64().unwrap());
                    let definition = evidence["cases"].as_array().unwrap().iter()
                        .find(|definition| definition["id"].as_str() == Some(case.id.as_str())).unwrap();
                    let matches: Vec<_> = definition["required_detail_patterns"].as_array().unwrap().iter()
                        .map(|pattern| regex::RegexBuilder::new(pattern.as_str().unwrap()).case_insensitive(true).build().unwrap().is_match(text))
                        .collect();
                    assert_eq!(matches.iter().all(|matched| *matched), output["all_detail_patterns_present"].as_bool().unwrap());
                }
            }
            assert_eq!(passed as u64, evidence["summary"][phase]["strict_contract_passed"].as_u64().unwrap());
        }
    }

    #[test]
    fn actual_text_input_quality_grades_retain_real_outcomes_and_consistent_scores() {
        let evidence: serde_json::Value = serde_json::from_str(include_str!("../../../eval/graph-quality-graded-results.json")).unwrap();
        assert_eq!(evidence["model"], "qwen3.5-9b-q4km");
        let dimensions = evidence["rubric"]["dimensions"].as_array().unwrap();
        assert_eq!(dimensions.len(), 5);
        let outputs = evidence["results"].as_array().unwrap();
        assert_eq!(outputs.len(), 18);
        let mut seen = std::collections::BTreeSet::new();
        let mut returned = 0;
        for output in outputs {
            let id = output["case"]["id"].as_str().unwrap();
            let rendering = output["rendering"].as_str().unwrap();
            assert!(seen.insert((id, rendering)));
            assert!(output["case"]["said"].as_str().is_some_and(|said| !said.is_empty()));
            assert!(output["command"].as_array().is_some_and(|args| args.iter().any(|arg| arg == "rewrite")));
            let outcome = &output["report"]["outcome"];
            let text = outcome["text"].as_str();
            if let Some(text) = text {
                assert!(score(Expect::Graph, Some(text)).pass, "{id}/{rendering}");
                returned += 1;
            } else {
                assert_eq!(outcome["kind"], "failed");
                assert!(output["stderr"].as_str().is_some_and(|detail| detail.contains("Rejected repair:")));
            }

            let grade = &output["grade"];
            let sum: u64 = dimensions.iter().map(|dimension| {
                let score = grade[dimension.as_str().unwrap()].as_u64().unwrap();
                assert!(score <= 5);
                if text.is_none() { assert_eq!(score, 0); }
                score
            }).sum();
            assert_eq!(grade["overall_10"].as_f64().unwrap(), sum as f64 * 2.0 / 5.0);
            assert!(grade["evidence"].as_str().is_some_and(|detail| !detail.is_empty()));
        }
        assert_eq!(returned, 17);
    }

    #[test]
    fn quality_suite_covers_original_heldout_and_dictation_cases() {
        let suite = load_quality_suite(include_str!("../../../eval/quality-review.toml")).unwrap();
        assert_eq!(suite.prompt.len(), 9);
        assert_eq!(suite.heldout_prompt.len(), 8);
        assert_eq!(suite.dictation.len(), 3);
        assert!(suite.dictation.iter().all(|case| !case.said.is_empty()));
        assert!(suite.heldout_prompt.iter().any(|case| case.category == "missing_reference"));
        assert!(suite.heldout_prompt.iter().any(|case| case.category == "post_verification_edit"));
        assert!(load_quality_suite("[[prompt]]\nid='same'\ncategory='x'\nsaid='x'\n[[heldout_prompt]]\nid='same'\ncategory='x'\nsaid='x'\n[[dictation]]\nid='d'\ncategory='x'\nsaid='x'").is_err());

        for (fragment, expected_id) in [
            (include_str!("../../../eval/quality-review-heldout-batch-1.toml"), "b1_h09"),
            (include_str!("../../../eval/quality-review-heldout-batch-2.toml"), "b2_h11"),
            (include_str!("../../../eval/quality-review-heldout-batch-3.toml"), "b3_h13"),
            (include_str!("../../../eval/quality-review-heldout-batch-4.toml"), "b4_h15"),
            (include_str!("../../../eval/quality-review-heldout-batch-5.toml"), "b5_h17"),
            (include_str!("../../../eval/quality-review-heldout-batch-6.toml"), "b6_h19"),
            (include_str!("../../../eval/quality-review-heldout-batch-7.toml"), "b7_h21"),
        ] {
            let combined = format!("{}\n\n{fragment}", include_str!("../../../eval/quality-review.toml"));
            let expanded = load_quality_suite(&combined).unwrap();
            assert_eq!(expanded.prompt.len(), 9);
            let expected_heldout_count = match expected_id {
                "b1_h09" => 10,
                "b2_h11" => 12,
                "b3_h13" => 14,
                "b4_h15" => 16,
                "b5_h17" => 18,
                "b6_h19" => 20,
                _ => 22,
            };
            assert_eq!(expanded.heldout_prompt.len(), expected_heldout_count);
            assert!(expanded.heldout_prompt.iter().any(|case| case.id == expected_id));
            if expected_id == "b3_h13" {
                assert!(expanded.heldout_prompt.iter().any(|case| case.id == "b3_h14"));
            }
            if expected_id == "b4_h15" {
                assert!(expanded.heldout_prompt.iter().any(|case| case.id == "b4_h16"));
            }
            if expected_id == "b5_h17" {
                assert!(expanded.heldout_prompt.iter().any(|case| case.id == "b5_h18"));
            }
            if expected_id == "b6_h19" {
                assert!(expanded.heldout_prompt.iter().any(|case| case.id == "b6_h20"));
            }
            if expected_id == "b7_h21" {
                assert!(expanded.heldout_prompt.iter().any(|case| case.id == "b7_h22"));
            }
        }
    }

    #[test]
    fn scoring_requires_valid_structure_matching_expectation() {
        let graph = "Step 1: Draft the explanation.\nStep 2 (after 1): Check its accuracy.\nLoop: if Step 2 fails the accuracy checks, return to Step 1 to correct inaccuracies; then recheck Step 2 (max 2 rounds).\nDone when: the explanation is accurate.";
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
    fn continuation_fixture_is_explicit_and_not_implicit_history() {
        let source = format!("{}\n{}", include_str!("../../../eval/quality-review.toml"),
            include_str!("../../../eval/quality-review-heldout-continuation.toml"));
        let suite = load_quality_suite(&source).unwrap();
        let absent = suite.heldout_prompt.iter().find(|case| case.id == "c01").unwrap();
        let present = suite.heldout_prompt.iter().find(|case| case.id == "c02").unwrap();
        assert_eq!(absent.said, present.said);
        assert!(absent.previous_prompt.is_none());
        assert!(present.previous_prompt.as_deref().unwrap().contains("two consecutive"));
        assert!(suite.prompt.iter().all(|case| case.previous_prompt.is_none()));
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
