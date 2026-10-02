//! Offline evaluation of prompt structure against recorded requests.

use serde::Deserialize;

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
    fn bundled_cases_parse_with_opaque_ids() {
        let cases = load_cases(include_str!("../../../eval/cases.toml")).unwrap();
        assert!(cases.len() >= 20);
        assert!(cases.iter().any(|c| c.expect == Expect::Graph) && cases.iter().any(|c| c.expect == Expect::Flat));
        assert!(cases.iter().all(|c| c.id.len() <= 4 && c.id.starts_with('c')));
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
