//! Target and input-surface contracts for opt-in prompt rewriting.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::context::ActiveContext;
use crate::profiles::{NewlinePolicy, Profile, ProfileKind, ProfileSet};
use crate::structure::validate_graph;

pub const CATALOG_VERSION: u32 = 1;
const UNCONFIRMED_INPUT_WARNING: &str = "The focused AI input is not confirmed. Review and copy the result, or explicitly select the input surface for this request.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rendering {
    #[default]
    Legacy,
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptForm {
    Graph,
    InlineGraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Chat,
    CodeChat,
    ResearchChat,
    SourceChat,
    DocumentChat,
    SpreadsheetChat,
    SqlChat,
    BuilderChat,
    PresentationChat,
    ImagePrompt,
    VideoPrompt,
    MusicDescription,
    MusicStyle,
    SoundPrompt,
    VoiceDesign,
    ObjectPrompt,
    TexturePrompt,
    SearchQuery,
    Literal,
    Unknown,
}

impl Surface {
    pub fn all() -> &'static [Self] {
        &[
            Self::Chat,
            Self::CodeChat,
            Self::ResearchChat,
            Self::SourceChat,
            Self::DocumentChat,
            Self::SpreadsheetChat,
            Self::SqlChat,
            Self::BuilderChat,
            Self::PresentationChat,
            Self::ImagePrompt,
            Self::VideoPrompt,
            Self::MusicDescription,
            Self::MusicStyle,
            Self::SoundPrompt,
            Self::VoiceDesign,
            Self::ObjectPrompt,
            Self::TexturePrompt,
            Self::SearchQuery,
            Self::Literal,
        ]
    }

    pub fn can_reply(self) -> bool {
        matches!(
            self,
            Self::Chat
                | Self::CodeChat
                | Self::ResearchChat
                | Self::SourceChat
                | Self::DocumentChat
                | Self::SpreadsheetChat
                | Self::SqlChat
                | Self::BuilderChat
                | Self::PresentationChat
                | Self::Unknown
        )
    }

    fn task_prefix(self) -> Option<&'static str> {
        match self {
            Self::ImagePrompt => Some("visual.image_generate"),
            Self::VideoPrompt => Some("visual.video_generate"),
            Self::MusicDescription | Self::MusicStyle => Some("audio."),
            Self::SoundPrompt => Some("audio."),
            Self::VoiceDesign => Some("audio.voice_design"),
            Self::ObjectPrompt | Self::TexturePrompt => Some("spatial."),
            Self::SearchQuery => Some("research."),
            _ => None,
        }
    }

    fn default_task(self) -> &'static str {
        match self {
            Self::ImagePrompt => "visual.image_generate",
            Self::VideoPrompt => "visual.video_generate",
            _ => "general.request",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskId(String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TaskId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parts: Vec<_> = value.split('.').collect();
        if value.len() > 80
            || parts.len() != 2
            || !parts.iter().all(|part| {
                part.as_bytes().first().is_some_and(u8::is_ascii_lowercase) && part.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            })
        {
            return Err("task ID must contain two lowercase identifiers separated by a dot".into());
        }
        Ok(Self(value))
    }
}

impl From<TaskId> for String {
    fn from(id: TaskId) -> Self {
        id.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutingOptions {
    pub rendering: Rendering,
    pub task_type: Option<TaskId>,
    pub surface: Option<Surface>,
}

impl RoutingOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.rendering == Rendering::Legacy && (self.task_type.is_some() || self.surface.is_some()) {
            return Err("task and surface overrides require adaptive rendering".into());
        }
        if self.surface == Some(Surface::Unknown) {
            return Err("unknown is a detection result, not a surface override".into());
        }
        if let Some(id) = &self.task_type {
            let task = catalog().get(id.as_str()).ok_or_else(|| format!("unknown prompt type: {}", id.as_str()))?;
            if task.status != RecipeStatus::Enabled {
                return Err(format!("{} is catalogued but not enabled", id.as_str()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeStatus {
    Proposed,
    Validated,
    Enabled,
    Retired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub id: TaskId,
    pub family: String,
    pub label: String,
    pub inputs: String,
    pub priority: String,
    pub feasibility: String,
    pub status: RecipeStatus,
    pub form: PromptForm,
    #[serde(default)]
    pub conversational: bool,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub rewrite: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub suppresses: Vec<TaskId>,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub negatives: Vec<String>,
    #[serde(skip)]
    rules: Vec<Regex>,
    #[serde(skip)]
    exclusions: Vec<Regex>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    version: u32,
    #[serde(rename = "task")]
    tasks: Vec<TaskDefinition>,
}

pub struct Catalog {
    tasks: Vec<TaskDefinition>,
}

impl Catalog {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut file: CatalogFile = toml::from_str(text).map_err(|e| format!("invalid prompt catalog: {e}"))?;
        if file.version != CATALOG_VERSION {
            return Err(format!("unsupported prompt catalog version: {}", file.version));
        }
        let mut ids = BTreeSet::new();
        for task in &mut file.tasks {
            if !ids.insert(task.id.clone()) {
                return Err(format!("duplicate prompt type: {}", task.id.as_str()));
            }
            if task.label.trim().is_empty()
                || task.inputs.trim().is_empty()
                || task.family.trim().is_empty()
                || !["P1", "P2", "P3", "B"].contains(&task.priority.as_str())
                || !["F1", "F2", "F3"].contains(&task.feasibility.as_str())
            {
                return Err(format!("invalid catalog metadata: {}", task.id.as_str()));
            }
            if task.status == RecipeStatus::Enabled
                && (task.instructions.trim().is_empty() || task.rewrite.trim().is_empty() || task.examples.len() < 4 || task.negatives.len() < 2)
            {
                return Err(format!("enabled prompt type needs instructions and evaluation cases: {}", task.id.as_str()));
            }
            if task.status == RecipeStatus::Enabled {
                validate_graph(&task.rewrite).map_err(|error| format!("{}: invalid graph example: {error}", task.id.as_str()))?;
            }
            let compile = |patterns: &[String]| -> Result<Vec<Regex>, String> {
                patterns
                    .iter()
                    .map(|pattern| {
                        RegexBuilder::new(pattern)
                            .case_insensitive(true)
                            .size_limit(1 << 20)
                            .build()
                            .map_err(|e| format!("{}: invalid routing pattern: {e}", task.id.as_str()))
                    })
                    .collect()
            };
            task.rules = compile(&task.patterns)?;
            task.exclusions = compile(&task.exclude)?;
        }
        for task in &file.tasks {
            for suppressed in &task.suppresses {
                if !ids.contains(suppressed) || *suppressed == task.id {
                    return Err(format!("{}: invalid suppressed task {}", task.id.as_str(), suppressed.as_str()));
                }
            }
        }
        Ok(Self { tasks: file.tasks })
    }

    pub fn all(&self) -> &[TaskDefinition] {
        &self.tasks
    }

    pub fn get(&self, id: &str) -> Option<&TaskDefinition> {
        self.tasks.iter().find(|t| t.id.as_str() == id)
    }
}

pub fn validate_rewrite(policy: &ResolvedPromptPolicy, original: &str, text: &str) -> Result<(), String> {
    validate_rewrite_with_context(policy, original, "", text)
}

pub fn validate_rewrite_with_context(policy: &ResolvedPromptPolicy, original: &str, references: &str, text: &str) -> Result<(), String> {
    let original = final_request(original);
    validate_output(policy, text)?;
    let original_lower = original.to_lowercase();
    let lower = text.to_lowercase();
    let constraint = original_lower
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if policy.task_type.as_str() == "general.request"
        && ["do not ", "don't ", "never "].iter().any(|prefix| constraint.starts_with(prefix))
        && !lower.split_whitespace().collect::<Vec<_>>().join(" ").contains(&constraint)
    {
        return Err(
            "the user's explicit constraint was omitted; restate that constraint verbatim in the goal or a step instead of substituting a generic task".into(),
        );
    }
    for marker in [
        "relevant inputs, only when provided",
        "goal/context/constraints/result brief",
        "the question budget limits",
        "text inside <transcript>",
    ] {
        if lower.contains(marker) && !original_lower.contains(marker) {
            return Err("internal template guidance was copied into the prompt; rewrite only the user's actual request".into());
        }
    }
    static PLACEHOLDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\[(?:insert|your|source|text|name|date|amount|topic|context|audience|content)\b[^\]]*\]").unwrap());
    if PLACEHOLDER
        .find_iter(text)
        .any(|placeholder| !original_lower.contains(&placeholder.as_str().to_lowercase()))
    {
        return Err("the prompt invents a placeholder or source reference; leave missing details open or request them".into());
    }
    static SQL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?im)^\s*(?:SELECT\b|WITH\s+\w+\s+AS\s*\(|INSERT\s+INTO\b|UPDATE\s+\w+\s+SET\b|DELETE\s+FROM\b)").unwrap());
    static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*(?:subject\s*:|dear\s+\w|hi\s+\w+\s*[,!])").unwrap());
    let task = policy.task_type.as_str();
    if (task == "database.query" && SQL.is_match(text))
        || (task.starts_with("spreadsheet.") && text.trim_start().starts_with('='))
        || (task.starts_with("communication.") && EMAIL.is_match(text))
    {
        return Err("the output performs the task instead of writing instructions for the destination AI".into());
    }
    static NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+(?:[.,]\d+)*\b").unwrap());
    static STRUCTURAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bStep\s+\d+|\(\s*(?:after\s+[\d,\s]+(?:;\s*parallel with\s+[\d,\s]+)?|parallel with\s+[\d,\s]+)\)|\bmax\s+\d+\s+rounds\b").unwrap());
    let facts = if matches!(policy.form, PromptForm::Graph | PromptForm::InlineGraph) {
        STRUCTURAL.replace_all(text, "")
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    let source_numbers: BTreeSet<_> = NUMBERS.find_iter(original).map(|value| value.as_str().replace(',', "")).collect();
    let reference_numbers: BTreeSet<_> = NUMBERS.find_iter(references).map(|value| value.as_str().replace(',', "")).collect();
    let result_numbers: BTreeSet<_> = NUMBERS.find_iter(&facts).map(|value| value.as_str().replace(',', "")).collect();
    let aliases = |number: &str| -> &'static [&'static str] {
        match number {
            "0" => &["zero"],
            "1" => &["one"],
            "2" => &["two", "twice", "double"],
            "3" => &["three"],
            "4" => &["four"],
            "5" => &["five"],
            "6" => &["six"],
            "7" => &["seven"],
            "8" => &["eight"],
            "9" => &["nine"],
            "10" => &["ten"],
            "11" => &["eleven"],
            "12" => &["twelve"],
            "20" => &["twenty"],
            "30" => &["thirty"],
            "60" => &["sixty"],
            "100" => &["hundred"],
            _ => &[],
        }
    };
    let has_alias = |value: &str, number: &str| value.split(|c: char| !c.is_alphanumeric()).any(|word| aliases(number).contains(&word));
    for number in &result_numbers {
        if !source_numbers.contains(number)
            && !reference_numbers.contains(number)
            && !has_alias(&original_lower, number)
            && !has_alias(&references.to_lowercase(), number)
            && !policy.max_chars.is_some_and(|limit| number == &limit.to_string())
        {
            return Err(format!("the prompt introduces the number {number}, which the user did not supply; remove invented numeric facts and constraints, and use only valid numeric dependency clauses"));
        }
    }
    let user_content = crate::structure::without_graph_metadata(text);
    let facts_lower = user_content.to_lowercase();
    let content_numbers: BTreeSet<_> = NUMBERS.find_iter(&user_content).map(|value| value.as_str().replace(',', "")).collect();
    for number in &source_numbers {
        if !content_numbers.contains(number) && !has_alias(&facts_lower, number) {
            return Err("a stated number is missing; preserve the user's numeric constraints and facts".into());
        }
    }
    Ok(())
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: LazyLock<Catalog> =
        LazyLock::new(|| Catalog::from_toml(include_str!("../profiles/prompt-types.toml")).expect("bundled routing catalog is validated by tests"));
    &CATALOG
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionReason {
    Explicit,
    Matched,
    SurfaceDefault,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPromptPolicy {
    pub version: u32,
    pub target_profile_id: String,
    pub target_name: String,
    pub surface: Surface,
    pub task_type: TaskId,
    pub task_label: String,
    pub secondary_tasks: Vec<TaskId>,
    pub form: PromptForm,
    pub conversational: bool,
    pub reason: ResolutionReason,
    pub auto_paste: bool,
    pub warnings: Vec<String>,
    pub max_chars: Option<usize>,
    pub newlines: NewlinePolicy,
}

impl ResolvedPromptPolicy {
    pub fn confirm_destination(&mut self) {
        if self.surface.can_reply() {
            self.auto_paste = true;
            self.warnings.retain(|warning| warning != UNCONFIRMED_INPUT_WARNING);
        }
    }

    pub fn tasks(&self) -> impl Iterator<Item = &TaskDefinition> {
        std::iter::once(&self.task_type)
            .chain(&self.secondary_tasks)
            .filter_map(|id| catalog().get(id.as_str()))
    }

    pub fn compatible_with(&self, other: &Self) -> bool {
        self.version == other.version
            && self.target_profile_id == other.target_profile_id
            && self.surface == other.surface
            && self.task_type == other.task_type
            && self.form == other.form
    }
}

fn host_is(host: Option<&str>, expected: &str) -> bool {
    host.is_some_and(|h| h == expected || h.strip_suffix(expected).is_some_and(|s| s.ends_with('.')))
}

fn detected_surface(ctx: &ActiveContext, profile: &Profile) -> (Surface, String, String, bool) {
    let host = ctx.url_host();
    let host = host.as_deref();
    let process = ctx.normalized_process();
    let specialist = [
        ("suno.com", "suno", "Suno"),
        ("elevenlabs.io", "elevenlabs", "ElevenLabs"),
        ("meshy.ai", "meshy", "Meshy"),
        ("notion.so", "notion", "Notion"),
        ("notion.com", "notion", "Notion"),
        ("office.com", "office", "Microsoft 365"),
        ("m365.cloud.microsoft", "office", "Microsoft 365"),
        ("figma.com", "figma", "Figma"),
    ];
    for (domain, id, name) in specialist {
        if host_is(host, domain) {
            return (Surface::Unknown, id.into(), name.into(), false);
        }
    }
    if ["excel", "winword", "powerpnt", "outlook", "notion"].contains(&process.as_str()) {
        return (Surface::Unknown, process.clone(), process, false);
    }
    // These sites are prompt-driven, but their settings/editors can still have focus.
    for (domain, id, name, surface) in [
        ("v0.app", "v0", "v0", Surface::BuilderChat),
        ("v0.dev", "v0", "v0", Surface::BuilderChat),
        ("lovable.dev", "lovable", "Lovable", Surface::BuilderChat),
        ("bolt.new", "bolt", "Bolt", Surface::BuilderChat),
        ("gamma.app", "gamma", "Gamma", Surface::PresentationChat),
        ("notebooklm.google.com", "notebook", "Notebook", Surface::SourceChat),
        ("notebook.google.com", "notebook", "Notebook", Surface::SourceChat),
    ] {
        if host_is(host, domain) {
            return (surface, id.into(), name.into(), false);
        }
    }
    let surface = match profile.kind {
        ProfileKind::ImageGen => Surface::ImagePrompt,
        ProfileKind::VideoGen => Surface::VideoPrompt,
        ProfileKind::CodeAgent => Surface::CodeChat,
        ProfileKind::Search => Surface::ResearchChat,
        ProfileKind::AiChat if profile.id != "generic" => Surface::Chat,
        _ => Surface::Unknown,
    };
    let auto_paste = !matches!(surface, Surface::Unknown) && profile.directly_matches(ctx) && !["cursor", "vscode", "terminal"].contains(&profile.id.as_str());
    (surface, profile.id.clone(), profile.name.clone(), auto_paste)
}

pub fn is_ai_target(ctx: &ActiveContext, profile: &Profile) -> bool {
    detected_surface(ctx, profile).0 != Surface::Unknown
}

fn classification_text(text: &str) -> String {
    let text = final_request(text);
    let mut quote = None;
    text.chars()
        .map(|ch| {
            if ch == '"' || ch == '`' {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
                ' '
            } else if quote.is_some() {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// Only a clearly restarted task is substituted; incremental edits retain their earlier context.
pub fn final_request(text: &str) -> &str {
    static RESTART: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)\b(?:no wait|scratch that|actually)[,:\s]+(?P<task>(?:translate|summarize|summarise|debug|refactor|classify|extract|research|brainstorm|proofread|write|draft|create|generate|build|implement)\b)"
    ).unwrap()
    });
    for captures in RESTART.captures_iter(text).collect::<Vec<_>>().into_iter().rev() {
        let start = captures.get(0).unwrap().start();
        let prefix = &text[..start];
        if !prefix.matches('"').count().is_multiple_of(2) || !prefix.matches('`').count().is_multiple_of(2) {
            continue;
        }
        let Some(task) = captures.name("task") else { continue };
        return &text[task.start()..];
    }
    text
}

pub fn is_follow_up(text: &str) -> bool {
    static FOLLOW_UP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*(?:also\b|(?:make|shorten|rewrite|change) (?:it|this|that|the (?:email|text|prompt|draft|response))\b|instead\b|add (?:that|this)\b)").unwrap()
    });
    FOLLOW_UP.is_match(text)
}

fn matches_task(task: &TaskDefinition, text: &str) -> Option<usize> {
    static NEGATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:do not|don't|never|not|without|rather than)\s+(?:\w+\s+){0,2}$").unwrap());
    if task.status != RecipeStatus::Enabled || task.exclusions.iter().any(|re| re.is_match(text)) {
        return None;
    }
    task.rules
        .iter()
        .flat_map(|rule| rule.find_iter(text))
        .filter(|m| !NEGATED.is_match(&text[..m.start()]))
        .map(|m| m.start())
        .min()
}

pub fn resolve(ctx: &ActiveContext, profile: &Profile, transcript: &str, options: &RoutingOptions) -> Result<ResolvedPromptPolicy, String> {
    options.validate()?;
    if transcript.trim().is_empty() || transcript.chars().count() > crate::transform::MAX_TEXT_INPUT_CHARS {
        return Err("the prompt request must contain between 1 and 8000 characters".into());
    }
    if options.rendering != Rendering::Adaptive {
        return Err("adaptive resolution requires adaptive rendering".into());
    }
    let (detected, target_profile_id, target_name, detected_auto_paste) = detected_surface(ctx, profile);
    let surface = options.surface.unwrap_or(detected);
    if surface == Surface::Literal {
        return Err("This is a literal-content field, not an AI prompt field. Use Dictation mode instead.".into());
    }
    if matches!(detected, Surface::ImagePrompt | Surface::VideoPrompt) && surface != detected {
        return Err(
            "The selected surface conflicts with the identified generator. Select the actual destination rather than overriding its input contract.".into(),
        );
    }
    let text = classification_text(transcript);
    static NEXT_TASK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)(?:[;,.]|\b(?:and(?: then)?|then|also|plus))\s+(?P<action>write|draft|add|create|generate|build|implement|explain|debug|fix|review|refactor|summarize|summarise|translate|analyze|analyse|extract|classify|research|make|plan|list)\b"
    ).unwrap()
    });
    let mut starts = vec![0];
    starts.extend(NEXT_TASK.captures_iter(&text).filter_map(|captures| captures.name("action").map(|m| m.start())));
    let mut matched = Vec::new();
    for (clause, start) in starts.iter().copied().enumerate() {
        let end = starts.get(clause + 1).copied().unwrap_or(text.len());
        for task in catalog().all() {
            if let Some(pos) = matches_task(task, &text[start..end]) {
                matched.push((task, start + pos, clause));
            }
        }
    }
    let suppressed: BTreeSet<_> = matched
        .iter()
        .flat_map(|(task, _, clause)| task.suppresses.iter().cloned().map(|id| (id, *clause)))
        .collect();
    matched.retain(|(task, _, clause)| !suppressed.contains(&(task.id.clone(), *clause)));
    matched.sort_by_key(|(_, pos, _)| *pos);
    let mut seen = BTreeSet::new();
    let mut clauses = BTreeSet::new();
    matched.retain(|(task, _, clause)| clauses.insert(*clause) && seen.insert(task.id.clone()));

    if let Some(prefix) = surface.task_prefix() {
        if let Some(id) = &options.task_type {
            if !id.as_str().starts_with(prefix) {
                return Err(format!("{} is incompatible with the selected {surface:?} surface", id.as_str()));
            }
        } else if matched.iter().any(|(task, _, _)| !task.id.as_str().starts_with(prefix)) {
            return Err(format!(
                "This request does not match the {surface:?} input. Choose the actual AI surface or use Dictation."
            ));
        }
        matched.retain(|(task, _, _)| task.id.as_str().starts_with(prefix));
    }
    let explicit = options.task_type.as_ref().and_then(|id| catalog().get(id.as_str()));
    let primary = explicit
        .or_else(|| matched.first().map(|(task, _, _)| *task))
        .or_else(|| catalog().get(surface.default_task()));
    if primary.is_none()
        && ["do not ", "don't ", "never "]
            .iter()
            .any(|prefix| text.trim_start().to_lowercase().starts_with(prefix))
    {
        return Err("Only a restriction was provided, without a recognized task. Add what you want the AI to do while retaining that restriction; Promptify will not invent a goal.".into());
    }
    let mut warnings = Vec::new();
    let task_type = primary.map(|task| task.id.clone()).unwrap_or_else(|| TaskId("general.request".into()));
    let mut form = PromptForm::Graph;
    if matches!(surface, Surface::Chat | Surface::CodeChat | Surface::ResearchChat | Surface::Unknown) && task_type.as_str().starts_with("visual.") {
        warnings.push("The destination's media-generation capability is unknown; this is a request, not a capability guarantee.".into());
    }
    let reason = if explicit.is_some() {
        ResolutionReason::Explicit
    } else if !matched.is_empty() {
        ResolutionReason::Matched
    } else if primary.is_some() {
        ResolutionReason::SurfaceDefault
    } else {
        warnings.push("No enabled task rule matched; using a general task graph without inventing a task type.".into());
        ResolutionReason::Uncertain
    };
    let auto_paste = (options.surface.is_some() || detected_auto_paste) && surface.can_reply();
    if !surface.can_reply() {
        warnings.push("This generator/query field cannot be assumed to execute the required graph and check loop. Review the workflow and use a conversational AI with the appropriate tools; it will not be pasted automatically.".into());
    }
    if !auto_paste {
        warnings.push(UNCONFIRMED_INPUT_WARNING.into());
    }
    let newlines = if !surface.can_reply() || profile.newlines == NewlinePolicy::Collapse {
        NewlinePolicy::Collapse
    } else {
        NewlinePolicy::Keep
    };
    if form == PromptForm::Graph && newlines == NewlinePolicy::Collapse {
        form = PromptForm::InlineGraph;
    }
    if matched.len() > 8 {
        return Err("More than eight distinct prompt tasks were recognized. Split this request into smaller prompts.".into());
    }
    let secondary_tasks = matched
        .into_iter()
        .filter(|(_, _, clause)| explicit.is_none() || *clause > 0)
        .map(|(task, _, _)| task.id.clone())
        .filter(|id| *id != task_type)
        .collect();
    Ok(ResolvedPromptPolicy {
        version: CATALOG_VERSION,
        target_profile_id,
        target_name,
        surface,
        task_type,
        task_label: primary.map_or("General request".into(), |task| task.label.clone()),
        secondary_tasks,
        form,
        conversational: primary.is_some_and(|task| task.conversational),
        reason,
        auto_paste,
        warnings,
        max_chars: (surface == Surface::SoundPrompt).then_some(450),
        newlines,
    })
}

pub fn prepare_profile(
    profiles: &ProfileSet,
    profile: &Profile,
    ctx: &ActiveContext,
    transcript: &str,
    options: &RoutingOptions,
) -> Result<(Profile, Option<ResolvedPromptPolicy>), String> {
    options.validate()?;
    if options.rendering == Rendering::Adaptive {
        return resolve(ctx, profile, transcript, options).map(|policy| {
            let mut prepared = profile.clone();
            prepared.structure = if policy.newlines == NewlinePolicy::Collapse {
                crate::structure::Structure::Inline
            } else {
                crate::structure::Structure::Graph
            };
            (prepared, Some(policy))
        });
    }
    let media = if profile.kind == ProfileKind::AiChat {
        crate::prompt::media_request(transcript).and_then(|kind| profiles.media_request(profile, kind))
    } else {
        None
    };
    let mut prepared = media.unwrap_or_else(|| profile.clone());
    prepared.newlines = profile.newlines;
    prepared.structure = if prepared.newlines == NewlinePolicy::Collapse {
        crate::structure::Structure::Inline
    } else {
        crate::structure::Structure::Graph
    };
    if prepared.kind.is_media() {
        let id = if prepared.kind == ProfileKind::ImageGen {
            "visual.image_generate"
        } else {
            "visual.video_generate"
        };
        let task = catalog().get(id).expect("bundled media graph recipe");
        prepared.style = "Keep the complete task graph. Put the requested visual description, motion, style and constraints inside the creation step. Ask the recipient to use available media tools and report capability or verification limits honestly.".into();
        prepared.examples = vec![crate::profiles::Example {
            said: task.examples[0].clone(),
            prompt: task.rewrite.clone(),
        }];
        if prepared.newlines == NewlinePolicy::Collapse {
            prepared.examples[0].prompt = prepared.examples[0].prompt.lines().collect::<Vec<_>>().join("; ");
        }
    }
    Ok((prepared, None))
}

pub fn validate_output(policy: &ResolvedPromptPolicy, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("the generated prompt is empty".into());
    }
    if policy.max_chars.is_some_and(|limit| text.chars().count() > limit) {
        return Err(format!(
            "the prompt exceeds the selected surface's {} character limit",
            policy.max_chars.unwrap()
        ));
    }
    if policy.newlines == NewlinePolicy::Collapse && text.contains(['\n', '\r']) {
        return Err("this input surface requires a single line".into());
    }
    if text.contains("<|im_start|>") || text.contains("<|im_end|>") || text.contains("<think>") {
        return Err("the generated prompt contains model control markup".into());
    }
    validate_graph(text).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::ProfileSet;

    fn route(text: &str) -> ResolvedPromptPolicy {
        let profiles = ProfileSet::bundled();
        let ctx = ActiveContext {
            process_name: "chrome.exe".into(),
            url: Some("https://chatgpt.com".into()),
            ..Default::default()
        };
        resolve(
            &ctx,
            profiles.resolve(&ctx),
            text,
            &RoutingOptions {
                rendering: Rendering::Adaptive,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn catalog_counts_and_ids_are_stable() {
        let catalog = catalog();
        assert_eq!(catalog.all().len(), 264);
        assert_eq!(catalog.all().iter().filter(|t| t.priority == "P1").count(), 37);
        assert_eq!(catalog.all().iter().filter(|t| t.status == RecipeStatus::Enabled).count(), 39);
        assert!(TaskId::try_from("testing.e2e".to_owned()).is_ok());
        assert!(TaskId::try_from("../unsafe".to_owned()).is_err());
    }

    #[test]
    fn enabled_recipes_have_positive_and_confusable_negative_cases() {
        let mut failures = Vec::new();
        for task in catalog().all().iter().filter(|t| t.status == RecipeStatus::Enabled) {
            for example in &task.examples {
                let actual = route(example).task_type;
                if actual != task.id {
                    failures.push(format!("{example}: expected {}, got {}", task.id.as_str(), actual.as_str()));
                }
            }
            for example in &task.negatives {
                if route(example).task_type == task.id {
                    failures.push(format!("{example}: unexpected {}", task.id.as_str()));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn preserves_multiple_tasks_and_ignores_quoted_or_negated_keywords() {
        let policy = route("Debug the checkout crash and add unit tests");
        assert_eq!(policy.task_type.as_str(), "code.debug");
        assert!(policy.secondary_tasks.iter().any(|id| id.as_str() == "testing.unit"));
        let profiles = ProfileSet::bundled();
        assert!(
            resolve(
                &ActiveContext::default(),
                profiles.get("generic").unwrap(),
                "Do not summarize this article",
                &RoutingOptions {
                    rendering: Rendering::Adaptive,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("restriction")
        );
        assert_eq!(route("Explain the word \"debug\"").task_type.as_str(), "info.explain");
        assert_eq!(route("Explain the phrase \"no wait create a video\"").task_type.as_str(), "info.explain");
        assert_eq!(
            route("Write an email, no wait translate this into French").task_type.as_str(),
            "language.translate"
        );
        let policy = route("Implement a search function and write unit tests");
        assert_eq!(policy.task_type.as_str(), "code.implement");
        assert!(policy.secondary_tasks.iter().any(|id| id.as_str() == "testing.unit"));
        let policy = route("Translate this into French and summarize it");
        assert_eq!(policy.task_type.as_str(), "language.translate");
        assert!(policy.secondary_tasks.iter().any(|id| id.as_str() == "summary.brief"));
        let policy = route("Translate this research report into French");
        assert_eq!(policy.task_type.as_str(), "language.translate");
        assert!(policy.secondary_tasks.is_empty(), "mentioned topics must not become extra tasks");
        assert!(is_follow_up("Shorten the draft"));
        assert!(!is_follow_up("Write a new email"));
    }

    #[test]
    fn surfaces_are_not_inferred_from_a_mixed_purpose_brand() {
        let profiles = ProfileSet::bundled();
        for host in ["suno.com", "elevenlabs.io", "notion.so", "office.com"] {
            let ctx = ActiveContext {
                url: Some(format!("https://{host}/create?secret=private")),
                ..Default::default()
            };
            let policy = resolve(
                &ctx,
                profiles.resolve(&ctx),
                "help with this request",
                &RoutingOptions {
                    rendering: Rendering::Adaptive,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(policy.surface, Surface::Unknown);
            assert!(!policy.auto_paste);
            assert!(!serde_json::to_string(&policy).unwrap().contains("secret"));
        }
    }

    #[test]
    fn explicit_invalid_and_deferred_types_are_errors() {
        let mut options = RoutingOptions {
            task_type: Some(TaskId("code.debug".into())),
            ..Default::default()
        };
        assert!(options.validate().is_err());
        options.rendering = Rendering::Adaptive;
        assert!(options.validate().is_ok());
        options.task_type = Some(TaskId("spatial.cad".into()));
        assert!(options.validate().is_err());
        options.task_type = Some(TaskId("unknown.type".into()));
        assert!(options.validate().is_err());
    }

    #[test]
    fn literal_and_incompatible_surfaces_are_rejected() {
        let profiles = ProfileSet::bundled();
        let ctx = ActiveContext::default();
        let profile = profiles.resolve(&ctx);
        for surface in [Surface::Literal, Surface::ImagePrompt, Surface::VideoPrompt, Surface::VoiceDesign] {
            assert!(
                resolve(
                    &ctx,
                    profile,
                    "Debug the checkout crash",
                    &RoutingOptions {
                        rendering: Rendering::Adaptive,
                        surface: Some(surface),
                        ..Default::default()
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn does_not_match_lookalike_domains() {
        let profiles = ProfileSet::bundled();
        let ctx = ActiveContext {
            url: Some("https://suno.com.evil.example".into()),
            ..Default::default()
        };
        let policy = resolve(
            &ctx,
            profiles.resolve(&ctx),
            "hello",
            &RoutingOptions {
                rendering: Rendering::Adaptive,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(policy.target_profile_id, "generic");
        let ctx = ActiveContext {
            process_name: "chrome.exe".into(),
            window_title: "About ChatGPT".into(),
            url: Some("https://example.org".into()),
            ..Default::default()
        };
        let policy = resolve(
            &ctx,
            profiles.resolve(&ctx),
            "Write an email",
            &RoutingOptions {
                rendering: Rendering::Adaptive,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!policy.auto_paste, "a website title is not proof of a matching AI site");
    }

    #[test]
    fn validates_graph_mandate_and_exact_surface_length() {
        let mut policy = route("Write an email asking for the meeting notes");
        assert!(validate_output(&policy, "Write a concise email requesting the meeting notes.").is_err());
        assert!(validate_output(&policy, "Step 1: Write an email.\nLoop: check.\nDone when: done.").is_err());
        policy.surface = Surface::SoundPrompt;
        policy.form = PromptForm::InlineGraph;
        policy.newlines = NewlinePolicy::Collapse;
        policy.max_chars = Some(450);
        let graph = "Make a sound. Step 1: Create it; Step 2 (after 1): Check its source and texture; Loop: if Step 2 fails the source or texture checks, return to Step 1 to correct and regenerate it; then recheck Step 2 (max 2 rounds); Done when: the sound matches the stated source and texture. ";
        let exact = format!("{graph}{}", "a".repeat(450 - graph.chars().count()));
        assert_eq!(exact.chars().count(), 450);
        assert!(validate_output(&policy, &exact).is_ok());
        assert!(validate_output(&policy, &format!("{exact}a")).is_err());
        assert!(validate_output(&policy, "What sound would you like?").is_err());
    }

    #[test]
    fn rewrite_guards_reject_invented_numbers_and_allow_supplied_context() {
        let policy = route("Draft an article about our library");
        let graph = |body: &str| {
            format!(
                "Draft an article about our library.\nStep 1: {body}\nStep 2 (after 1): Check the facts.\nLoop: if Step 2 fails the factual support checks, return to Step 1 to correct unsupported facts; then recheck Step 2 (max 2 rounds).\nDone when: all stated facts are supported."
            )
        };
        let invented = graph("Say the library is at 123 Main Street.");
        assert!(validate_rewrite(&policy, "Draft an article about our library", &invented).unwrap_err().contains("number 123"));
        assert!(validate_rewrite_with_context(&policy, "Draft an article about our library", "The library is at 123 Main Street.", &invented).is_ok());
        let missing = graph("Write the draft concisely.");
        assert!(validate_rewrite(&policy, "Draft a 100 word article about our library", &missing).is_err());
        assert!(validate_rewrite(&policy, "Draft 2 articles about our library", &missing).is_err(), "a Step 2 label does not preserve two requested articles");
        let original = "Explain Step 2 of the installation guide";
        let explanation = "Explain Step 2 of the installation guide.\nStep 1: Explain the named guide step using the supplied guide.\nStep 2 (after 1): Verify the explanation against the guide.\nLoop: if Step 2 fails the explanation fidelity checks, return to Step 1 to correct the explanation; then recheck Step 2 (max 2 rounds).\nDone when: the explanation matches the named installation guide step.";
        assert!(validate_rewrite(&route(original), original, explanation).is_ok(), "a step reference in the user's goal is a supplied fact, not graph metadata");
    }

    #[test]
    fn parallel_and_loop_numbers_are_workflow_not_invented_facts() {
        let original = "Compare the supplied options";
        let policy = route(original);
        let graph = "Compare the supplied options.\nStep 1 (parallel with 2): Research the first supplied option.\nStep 2 (parallel with 1): Research the other supplied option.\nStep 3 (after 1, 2): Compare evidence.\nStep 4 (after 3): Verify the evidence and comparison.\nLoop: if Step 4 fails the evidence checks, return to Step 1 to repair evidence and update the comparison; then recheck Step 4 (max 2 rounds).\nDone when: the comparison is supported by evidence for the supplied options.";
        assert!(validate_rewrite(&policy, original, graph).is_ok());
    }

    #[test]
    fn enabled_examples_have_specific_checks_and_no_invented_numeric_facts() {
        for task in catalog().all().iter().filter(|task| task.status == RecipeStatus::Enabled) {
            let policy = route(&task.examples[0]);
            validate_rewrite(&policy, &task.examples[0], &task.rewrite).unwrap_or_else(|error| panic!("{}: {error}", task.id.as_str()));
            let summary = validate_graph(&task.rewrite).unwrap();
            let range = match crate::structure::complexity(&task.examples[0]) {
                crate::structure::Complexity::Simple => 2..=2,
                crate::structure::Complexity::Moderate => 3..=4,
                crate::structure::Complexity::Complex => 4..=8,
            };
            assert!(range.contains(&summary.steps), "{}: {summary:?} outside {range:?}", task.id.as_str());
            assert!(!task.rewrite.contains("The request is fulfilled faithfully"));
            assert!(!task.rewrite.contains("return to Step 2 (max"));
        }
    }
}
