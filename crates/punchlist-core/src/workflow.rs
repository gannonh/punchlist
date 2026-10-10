//! The workflow: statuses, transitions, who may make each one, the gates they need, the
//! statuses locked to roles, and which statuses dispatch an agent with which prompt.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use toml::Spanned;

use crate::gate::Gate;

const DEFAULT_WORKFLOW_TOML: &str = include_str!("default/workflow.toml");
const DEFAULT_PROMPTS: [(&str, &str); 3] = [
    (SYSTEM_PROMPT, include_str!("default/prompts/system.md")),
    (
        "prompts/in_progress.md",
        include_str!("default/prompts/in_progress.md"),
    ),
    (
        "prompts/agent_review.md",
        include_str!("default/prompts/agent_review.md"),
    ),
];

/// The preamble shared by every run, under `.punchlist/`. Optional (ADR 0001).
pub const SYSTEM_PROMPT: &str = "prompts/system.md";

/// Statuses the runtime hard-codes: a claim moves an issue from `START` to `IN_PROGRESS`
/// and dispatches by the rule for `IN_PROGRESS`. A workflow must keep them.
pub const START: &str = "start";
pub const IN_PROGRESS: &str = "in_progress";

/// The events a transition's `on` may name; the server makes each from a GitHub delivery.
const EVENTS: [&str; 2] = ["pr_merged", "pr_closed_unmerged"];
/// What a transition's `require` may ask a move to carry.
const REQUIREMENTS: [&str; 1] = ["comment"];
/// The roles a workflow names. GitHub acts only through `on`.
const ROLES: [Role; 3] = [Role::Person, Role::Agent, Role::Runner];

static DEFAULT_WORKFLOW: LazyLock<Workflow> = LazyLock::new(|| {
    let prompts = DEFAULT_PROMPTS
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    Workflow::from_files(DEFAULT_WORKFLOW_TOML, prompts).expect("the built-in workflow is valid")
});

/// The built-in workflow a new workspace starts with (ADR 0010).
pub fn default_workflow() -> &'static Workflow {
    &DEFAULT_WORKFLOW
}

/// The JSON Schema for `workflow.toml`, for editors with a TOML language server.
pub fn workflow_schema() -> schemars::Schema {
    schemars::schema_for!(RawWorkflow)
}

/// Who asks for a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Person,
    Agent,
    Runner,
    /// GitHub, through a webhook event. Makes only the transitions whose `on` names a
    /// GitHub event; its actors are named after the GitHub user who caused the event.
    Github,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Person, Role::Agent, Role::Runner, Role::Github];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Person => "person",
            Role::Agent => "agent",
            Role::Runner => "runner",
            Role::Github => "github",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == s)
    }

    fn with_article(self) -> &'static str {
        match self {
            Role::Person => "a person",
            Role::Agent => "an agent",
            Role::Runner => "a runner",
            Role::Github => "GitHub",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A status named in the workflow, such as `in_progress`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Status(String);

impl Status {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name people read: `in_progress` is "In Progress".
    pub fn display_name(&self) -> String {
        display_name(&self.0)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Turns a status key such as `in_progress` into "In Progress".
pub fn display_name(status: &str) -> String {
    status
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One allowed move. A transition with an empty `by` is made only by its `on` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub from: Vec<Status>,
    pub to: Status,
    pub by: Vec<Role>,
    pub on: Option<String>,
    /// Gates that must pass on recorded evidence before the move.
    pub gates: Vec<Gate>,
    /// What the move must carry, such as `comment`.
    pub require: Vec<String>,
}

/// Which agent a runner starts when an issue enters a status, and with which prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    pub status: Status,
    /// An agent key, such as `claude-code`.
    pub agent: String,
    /// A path under `.punchlist/`, such as `prompts/in_progress.md`.
    pub prompt: Option<String>,
    /// The line of `prompt` in the file, for an error about it.
    prompt_line: Option<usize>,
}

/// A parsed and validated workflow, with the prompt files it names.
#[derive(Debug, Clone)]
pub struct Workflow {
    version: String,
    source: String,
    statuses: Vec<Status>,
    transitions: Vec<Transition>,
    /// Each locked status and the roles that may not act on an issue in it.
    locks: Vec<(Status, Vec<Role>)>,
    dispatch: Vec<Dispatch>,
    prompts: BTreeMap<String, String>,
}

/// Why a workflow file did not load: the line, the key and the problem.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct LoadError {
    /// 1-based, when the problem has a place in the file.
    pub line: Option<usize>,
    /// The key path, such as `transition[3].gates[0]`; empty for the whole file.
    pub key: String,
    pub problem: Problem,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.key.is_empty()) {
            (Some(line), false) => write!(f, "line {line}, `{}`: {}", self.key, self.problem),
            (Some(line), true) => write!(f, "line {line}: {}", self.problem),
            (None, false) => write!(f, "`{}`: {}", self.key, self.problem),
            (None, true) => write!(f, "{}", self.problem),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("{0}")]
    Toml(String),
    #[error("version {0} is not supported; use version = 1")]
    UnsupportedVersion(i64),
    #[error("the workflow lists no statuses")]
    NoStatuses,
    #[error("status `{0}` is listed twice")]
    DuplicateStatus(String),
    #[error("status `{0}` is not in `statuses`")]
    UnknownStatus(String),
    #[error("`from` is a status or a list of statuses")]
    BadFrom,
    #[error("unknown role `{0}`; roles are person, agent and runner")]
    UnknownRole(String),
    #[error("the transition has neither `by` nor `on`")]
    NoActor,
    #[error("unknown event `{0}`; events are {events}", events = EVENTS.join(", "))]
    UnknownEvent(String),
    #[error("unknown gate `{0}`; gates are {gates}", gates = gate_names())]
    UnknownGate(String),
    #[error("unknown requirement `{0}`; requirements are {requirements}", requirements = REQUIREMENTS.join(", "))]
    UnknownRequirement(String),
    #[error("transition {from} → {to} is listed twice")]
    DuplicateTransition { from: String, to: String },
    #[error("the dispatch rule names no agent")]
    NoAgent,
    #[error("`max_concurrent` must be at least 1")]
    BadMaxConcurrent,
    #[error(
        "the runtime needs {0}; `start`, `in_progress`, the runner transition between them and the `in_progress` dispatch rule are reserved"
    )]
    MissingReserved(String),
    #[error("prompt path `{0}` must be relative to .punchlist/, such as prompts/in_progress.md")]
    BadPromptPath(String),
    #[error("prompt file `.punchlist/{0}` does not exist")]
    MissingPrompt(String),
}

fn gate_names() -> String {
    Gate::ALL.map(Gate::as_str).join(", ")
}

/// Why a transition was refused. The message names the rule that refused it.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum Refusal {
    #[error("status `{status}` is not in the workflow")]
    UnknownStatus { status: String },
    #[error("no transition {from} → {to} exists in the workflow")]
    NoTransition { from: String, to: String },
    #[error("{from} → {to} happens only on the `{on}` event, not on request")]
    EventOnly {
        from: String,
        to: String,
        on: String,
    },
    #[error("only {} makes {from} → {to}, not {}", role_list(allowed), role.with_article())]
    RoleNotAllowed {
        from: String,
        to: String,
        role: Role,
        allowed: Vec<Role>,
    },
    #[error("{status} is locked: {} cannot act on an issue in it", role.with_article())]
    Locked { status: String, role: Role },
    #[error("{from} → {to} needs gate {gate}: {reason}")]
    GateFailed {
        from: String,
        to: String,
        gate: String,
        reason: String,
    },
}

impl Refusal {
    /// A stable code for API clients.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::UnknownStatus { .. } => "unknown_status",
            Refusal::NoTransition { .. } => "no_transition",
            Refusal::EventOnly { .. } => "event_only",
            Refusal::RoleNotAllowed { .. } => "role_not_allowed",
            Refusal::Locked { .. } => "locked",
            Refusal::GateFailed { .. } => "gate_failed",
        }
    }
}

fn role_list(roles: &[Role]) -> String {
    let names: Vec<&str> = roles.iter().map(|role| role.with_article()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// Builds `LoadError`s for one file: turns byte spans into lines.
struct Errors<'a> {
    text: &'a str,
}

impl Errors<'_> {
    fn at(&self, span: Range<usize>, key: impl Into<String>, problem: Problem) -> LoadError {
        LoadError {
            line: Some(self.line(span.start)),
            key: key.into(),
            problem,
        }
    }

    fn line(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .matches('\n')
            .count()
            + 1
    }
}

impl Workflow {
    /// Parses and validates a `workflow.toml` without its prompt files.
    pub fn from_toml(text: &str) -> Result<Workflow, LoadError> {
        Workflow::from_files(text, BTreeMap::new()).or_else(|error| match error.problem {
            // A prompt is checked once the files are read.
            Problem::MissingPrompt(_) => Workflow::parse(text),
            _ => Err(error),
        })
    }

    /// Parses and validates a `workflow.toml` and the prompt files read beside it, keyed
    /// by their path under `.punchlist/`. Every prompt the workflow names must be there;
    /// `prompts/system.md` is kept when present. The version is a content hash over the
    /// workflow file and the prompts kept (ADR 0001).
    pub fn from_files(
        text: &str,
        mut files: BTreeMap<String, String>,
    ) -> Result<Workflow, LoadError> {
        let mut workflow = Workflow::parse(text)?;
        let mut prompts = BTreeMap::new();
        for rule in &workflow.dispatch {
            let Some(path) = &rule.prompt else { continue };
            match files.remove(path) {
                Some(prompt) => {
                    prompts.insert(path.clone(), prompt);
                }
                None if prompts.contains_key(path) => {}
                None => {
                    return Err(LoadError {
                        line: rule.prompt_line,
                        key: format!("dispatch.status.{}.prompt", rule.status),
                        problem: Problem::MissingPrompt(path.clone()),
                    });
                }
            }
        }
        if let Some(system) = files.remove(SYSTEM_PROMPT) {
            prompts.insert(SYSTEM_PROMPT.to_string(), system);
        }
        workflow.version = content_hash(text, &prompts);
        workflow.prompts = prompts;
        Ok(workflow)
    }

    fn parse(text: &str) -> Result<Workflow, LoadError> {
        let errors = Errors { text };
        let raw: RawWorkflow = toml::from_str(text).map_err(|e| LoadError {
            line: e.span().map(|span| errors.line(span.start)),
            key: String::new(),
            problem: Problem::Toml(e.message().to_string()),
        })?;
        if let Some(version) = raw.version
            && *version.get_ref() != 1
        {
            return Err(errors.at(
                version.span(),
                "version",
                Problem::UnsupportedVersion(*version.get_ref()),
            ));
        }
        if raw.statuses.get_ref().is_empty() {
            return Err(errors.at(raw.statuses.span(), "statuses", Problem::NoStatuses));
        }
        let statuses_span = raw.statuses.span();
        let mut statuses: Vec<Status> = Vec::new();
        for (i, name) in raw.statuses.into_inner().into_iter().enumerate() {
            if statuses.iter().any(|s| s.0 == *name.get_ref()) {
                let problem = Problem::DuplicateStatus(name.get_ref().clone());
                return Err(errors.at(name.span(), format!("statuses[{i}]"), problem));
            }
            statuses.push(Status(name.into_inner()));
        }
        let reserved = |key: &str, what: String| {
            errors.at(statuses_span.clone(), key, Problem::MissingReserved(what))
        };
        let known = |name: &str, span: Range<usize>, key: String| -> Result<Status, LoadError> {
            match statuses.iter().find(|s| s.0 == name) {
                Some(status) => Ok(status.clone()),
                None => Err(errors.at(span, key, Problem::UnknownStatus(name.to_string()))),
            }
        };
        let roles = |names: Vec<Spanned<String>>, key: &str| -> Result<Vec<Role>, LoadError> {
            names
                .into_iter()
                .enumerate()
                .map(|(j, name)| {
                    ROLES
                        .into_iter()
                        .find(|role| role.as_str() == name.get_ref())
                        .ok_or_else(|| {
                            let problem = Problem::UnknownRole(name.get_ref().clone());
                            errors.at(name.span(), format!("{key}[{j}]"), problem)
                        })
                })
                .collect()
        };

        let mut transitions: Vec<Transition> = Vec::new();
        for (i, t) in raw.transitions.into_iter().enumerate() {
            let key = |field: &str| format!("transition[{i}].{field}");
            let from_span = t.from.span();
            let names: Vec<String> = match t.from.into_inner() {
                toml::Value::String(name) => vec![name],
                toml::Value::Array(items) => items
                    .into_iter()
                    .map(|item| match item {
                        toml::Value::String(name) => Ok(name),
                        _ => Err(errors.at(from_span.clone(), key("from"), Problem::BadFrom)),
                    })
                    .collect::<Result<_, _>>()?,
                _ => return Err(errors.at(from_span, key("from"), Problem::BadFrom)),
            };
            let from = names
                .iter()
                .map(|name| known(name, from_span.clone(), key("from")))
                .collect::<Result<Vec<_>, _>>()?;
            let to = known(t.to.get_ref(), t.to.span(), key("to"))?;
            let by = roles(t.by, &key("by"))?;
            if by.is_empty() && t.on.is_none() {
                return Err(errors.at(t.to.span(), format!("transition[{i}]"), Problem::NoActor));
            }
            if let Some(on) =
                t.on.as_ref()
                    .filter(|on| !EVENTS.contains(&on.get_ref().as_str()))
            {
                let problem = Problem::UnknownEvent(on.get_ref().clone());
                return Err(errors.at(on.span(), key("on"), problem));
            }
            let mut gates = Vec::new();
            for (j, name) in t.gates.iter().enumerate() {
                let gate = Gate::parse(name.get_ref()).ok_or_else(|| {
                    let problem = Problem::UnknownGate(name.get_ref().clone());
                    errors.at(name.span(), key(&format!("gates[{j}]")), problem)
                })?;
                gates.push(gate);
            }
            for (j, name) in t.require.iter().enumerate() {
                if !REQUIREMENTS.contains(&name.get_ref().as_str()) {
                    let problem = Problem::UnknownRequirement(name.get_ref().clone());
                    return Err(errors.at(name.span(), key(&format!("require[{j}]")), problem));
                }
            }
            for f in &from {
                if transitions.iter().any(|o| o.to == to && o.from.contains(f)) {
                    let problem = Problem::DuplicateTransition {
                        from: f.0.clone(),
                        to: to.0.clone(),
                    };
                    return Err(errors.at(t.to.span(), format!("transition[{i}]"), problem));
                }
            }
            transitions.push(Transition {
                from,
                to,
                by,
                on: t.on.map(Spanned::into_inner),
                gates,
                require: t.require.into_iter().map(Spanned::into_inner).collect(),
            });
        }

        let mut locks = Vec::new();
        for (name, locked) in raw.lock {
            let key = format!("lock.{name}");
            let status = known(&name, locked.span(), key.clone())?;
            locks.push((status, roles(locked.into_inner(), &key)?));
        }

        if let Some(max) = raw.dispatch.max_concurrent.filter(|max| *max.get_ref() < 1) {
            let problem = Problem::BadMaxConcurrent;
            return Err(errors.at(max.span(), "dispatch.max_concurrent", problem));
        }
        let mut dispatch = Vec::new();
        for (name, rule) in raw.dispatch.status {
            let key = |field: &str| format!("dispatch.status.{name}.{field}");
            let status = known(&name, rule.agent.span(), format!("dispatch.status.{name}"))?;
            if rule.agent.get_ref().trim().is_empty() {
                return Err(errors.at(rule.agent.span(), key("agent"), Problem::NoAgent));
            }
            if let Some(prompt) = rule.prompt.as_ref().filter(|p| !is_relative(p.get_ref())) {
                let problem = Problem::BadPromptPath(prompt.get_ref().clone());
                return Err(errors.at(prompt.span(), key("prompt"), problem));
            }
            dispatch.push(Dispatch {
                status,
                agent: rule.agent.into_inner(),
                prompt_line: rule.prompt.as_ref().map(|p| errors.line(p.span().start)),
                prompt: rule.prompt.map(Spanned::into_inner),
            });
        }

        for name in [START, IN_PROGRESS] {
            if !statuses.iter().any(|s| s.0 == name) {
                return Err(reserved(
                    "statuses",
                    format!("status `{name}` in `statuses`"),
                ));
            }
        }

        if !transitions.iter().any(|t| {
            t.to.0 == IN_PROGRESS
                && t.from.iter().any(|f| f.0 == START)
                && t.by.contains(&Role::Runner)
        }) {
            let what = "a `[[transition]]` from `start` to `in_progress` with `by = [\"runner\"]`";
            return Err(reserved("transition", what.into()));
        }

        if !dispatch.iter().any(|d| d.status.0 == IN_PROGRESS) {
            let what = "a `[dispatch.status.in_progress]` rule".into();
            return Err(reserved("dispatch.status.in_progress", what));
        }

        Ok(Workflow {
            version: content_hash(text, &BTreeMap::new()),
            source: text.to_string(),
            statuses,
            transitions,
            locks,
            dispatch,
            prompts: BTreeMap::new(),
        })
    }

    /// A content hash over the workflow file and its prompts, recorded on every transition.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The `workflow.toml` text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The prompt files, keyed by their path under `.punchlist/`.
    pub fn prompts(&self) -> &BTreeMap<String, String> {
        &self.prompts
    }

    /// The prompt files to read beside `workflow.toml`: the system prompt and every
    /// prompt a dispatch rule names.
    pub fn prompt_paths(&self) -> Vec<String> {
        let mut paths = vec![SYSTEM_PROMPT.to_string()];
        for path in self.dispatch.iter().filter_map(|d| d.prompt.as_ref()) {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        paths
    }

    /// The prompt an agent dispatched for `status` starts with: the system prompt, then
    /// the status's own.
    pub fn prompt_for(&self, status: &str) -> String {
        let own = self
            .dispatch_for(status)
            .and_then(|d| d.prompt.as_ref())
            .and_then(|path| self.prompts.get(path));
        [self.prompts.get(SYSTEM_PROMPT), own]
            .into_iter()
            .flatten()
            .map(|text| text.trim_end())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn statuses(&self) -> &[Status] {
        &self.statuses
    }

    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }

    pub fn locks(&self) -> &[(Status, Vec<Role>)] {
        &self.locks
    }

    /// The dispatch rule for a status, if a runner starts an agent there.
    pub fn dispatch_for(&self, status: &str) -> Option<&Dispatch> {
        self.dispatch.iter().find(|d| d.status.0 == status)
    }

    /// The status a new issue starts in: the first one listed.
    pub fn initial_status(&self) -> &Status {
        &self.statuses[0]
    }

    pub fn status(&self, name: &str) -> Option<&Status> {
        self.statuses.iter().find(|s| s.0 == name)
    }

    /// May `role` act on an issue in `status`? The `[lock]` table says who may not.
    pub fn check_unlocked(&self, status: &str, role: Role) -> Result<(), Refusal> {
        if self
            .locks
            .iter()
            .any(|(locked, roles)| locked.0 == status && roles.contains(&role))
        {
            return Err(Refusal::Locked {
                status: status.to_string(),
                role,
            });
        }
        Ok(())
    }

    /// Applies the lock and `by` rules: may `role` move an issue from `from` to `to`? The
    /// caller then evaluates the transition's gates.
    pub fn check_transition(
        &self,
        from: &str,
        to: &str,
        role: Role,
    ) -> Result<&Transition, Refusal> {
        for status in [from, to] {
            if self.status(status).is_none() {
                return Err(Refusal::UnknownStatus {
                    status: status.to_string(),
                });
            }
        }
        self.check_unlocked(from, role)?;
        let transition = self
            .transitions
            .iter()
            .find(|t| t.to.0 == to && t.from.iter().any(|f| f.0 == from))
            .ok_or_else(|| Refusal::NoTransition {
                from: from.to_string(),
                to: to.to_string(),
            })?;
        if transition.by.contains(&role) {
            return Ok(transition);
        }
        match (&transition.on, transition.by.is_empty()) {
            (Some(on), true) => Err(Refusal::EventOnly {
                from: from.to_string(),
                to: to.to_string(),
                on: on.clone(),
            }),
            _ => Err(Refusal::RoleNotAllowed {
                from: from.to_string(),
                to: to.to_string(),
                role,
                allowed: transition.by.clone(),
            }),
        }
    }

    /// The transition an event makes from `from`, such as `merging → done` on `pr_merged`.
    /// `None` when the workflow has no transition on that event from that status: the event
    /// is recorded as evidence and the issue stays where it is.
    pub fn transition_on(&self, from: &str, event: &str) -> Option<&Transition> {
        self.transitions
            .iter()
            .find(|t| t.on.as_deref() == Some(event) && t.from.iter().any(|f| f.0 == from))
    }
}

/// A plain relative path with no `..`, so a prompt cannot name a file outside `.punchlist/`.
fn is_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// SHA-256 over each file's path, length and text: `workflow.toml`, then the prompts in
/// path order.
fn content_hash(text: &str, prompts: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    let files = std::iter::once(("workflow.toml", text))
        .chain(prompts.iter().map(|(p, t)| (p.as_str(), t.as_str())));
    for (path, content) in files {
        hasher.update(format!("{path}\0{}\0", content.len()));
        hasher.update(content);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

// The file as written. The `schemars` attributes give editors the names each list accepts.

/// A Punchlist workflow: `.punchlist/workflow.toml` (ADR 0001).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Punchlist workflow")]
struct RawWorkflow {
    /// The format version. Only 1.
    #[serde(default)]
    #[schemars(with = "Option<u32>")]
    version: Option<Spanned<i64>>,
    /// Every status, in order. A new issue starts in the first. `start` and `in_progress`
    /// are required.
    #[schemars(with = "Vec<String>")]
    statuses: Spanned<Vec<Spanned<String>>>,
    /// The allowed moves.
    #[serde(default, rename = "transition")]
    transitions: Vec<RawTransition>,
    /// Statuses locked to roles: an actor with a listed role cannot act on an issue in
    /// the status.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, Vec<RoleName>>")]
    lock: BTreeMap<String, Spanned<Vec<Spanned<String>>>>,
    #[serde(default)]
    dispatch: RawDispatch,
}

/// One allowed move: who may make it (`by`) or which event makes it (`on`), and what it needs.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawTransition {
    /// A status, or a list of statuses.
    #[schemars(with = "FromStatuses")]
    from: Spanned<toml::Value>,
    #[schemars(with = "String")]
    to: Spanned<String>,
    /// The roles that may request the move.
    #[serde(default)]
    #[schemars(with = "Vec<RoleName>")]
    by: Vec<Spanned<String>>,
    /// The GitHub event that makes the move.
    #[serde(default)]
    #[schemars(with = "Option<EventName>")]
    on: Option<Spanned<String>>,
    /// Checks over recorded evidence that must pass first.
    #[serde(default)]
    #[schemars(with = "Vec<GateName>")]
    gates: Vec<Spanned<String>>,
    /// What the move must carry.
    #[serde(default)]
    #[schemars(with = "Vec<RequirementName>")]
    require: Vec<Spanned<String>>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawDispatch {
    /// Issues dispatched at once.
    #[serde(default)]
    #[schemars(with = "Option<u32>")]
    max_concurrent: Option<Spanned<i64>>,
    /// The statuses that start an agent, keyed by status.
    #[serde(default)]
    status: BTreeMap<String, RawDispatchRule>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawDispatchRule {
    /// The agent to start, such as `claude-code` or `codex`.
    #[schemars(with = "String")]
    agent: Spanned<String>,
    /// The prompt file, relative to `.punchlist/`, such as `prompts/in_progress.md`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    prompt: Option<Spanned<String>>,
}

// Schema-only types: the names a field accepts.

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum FromStatuses {
    One(String),
    Many(Vec<String>),
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RoleName {
    Person,
    Agent,
    Runner,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EventName {
    PrMerged,
    PrClosedUnmerged,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum GateName {
    PrOpen,
    PrReady,
    PrNamesIssue,
    CiGreen,
    Mergeable,
    NoOpenThreads,
    ProofAttached,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RequirementName {
    Comment,
}
#[cfg(test)]
mod tests {
    use super::*;

    use Role::{Agent, Person, Runner};

    /// Every transition in the default workflow, crossed with every role. `Ok` means the
    /// role may make the move once its gates pass; otherwise the refusal code.
    const DEFAULT_RULES: &[(&str, &str, Role, Result<(), &str>)] = &[
        ("backlog", "todo", Person, Ok(())),
        ("backlog", "todo", Agent, Err("role_not_allowed")),
        ("backlog", "todo", Runner, Err("role_not_allowed")),
        ("todo", "start", Person, Ok(())),
        ("todo", "start", Agent, Err("role_not_allowed")),
        ("todo", "start", Runner, Err("role_not_allowed")),
        ("start", "in_progress", Person, Err("role_not_allowed")),
        ("start", "in_progress", Agent, Err("role_not_allowed")),
        ("start", "in_progress", Runner, Ok(())),
        (
            "in_progress",
            "agent_review",
            Person,
            Err("role_not_allowed"),
        ),
        ("in_progress", "agent_review", Agent, Ok(())),
        (
            "in_progress",
            "agent_review",
            Runner,
            Err("role_not_allowed"),
        ),
        (
            "agent_review",
            "human_review",
            Person,
            Err("role_not_allowed"),
        ),
        ("agent_review", "human_review", Agent, Ok(())),
        (
            "agent_review",
            "human_review",
            Runner,
            Err("role_not_allowed"),
        ),
        ("human_review", "merging", Person, Ok(())),
        ("human_review", "merging", Agent, Err("locked")),
        ("human_review", "merging", Runner, Err("locked")),
        ("merging", "done", Person, Err("event_only")),
        ("merging", "done", Agent, Err("event_only")),
        ("merging", "done", Runner, Err("event_only")),
        ("in_progress", "todo", Person, Err("event_only")),
        ("in_progress", "todo", Agent, Err("event_only")),
        ("in_progress", "todo", Runner, Err("event_only")),
        ("agent_review", "todo", Person, Err("event_only")),
        ("agent_review", "todo", Agent, Err("event_only")),
        ("agent_review", "todo", Runner, Err("event_only")),
    ];

    #[test]
    fn default_workflow_applies_by_rules() {
        let workflow = default_workflow();
        for &(from, to, role, expected) in DEFAULT_RULES {
            let actual = workflow
                .check_transition(from, to, role)
                .map(|_| ())
                .map_err(|refusal| refusal.code());
            assert_eq!(actual, expected, "{from} → {to} by {role}");
        }
    }

    #[test]
    fn rule_table_covers_every_default_transition() {
        let mut pairs: Vec<(String, String)> = default_workflow()
            .transitions()
            .iter()
            .flat_map(|t| t.from.iter().map(|f| (f.0.clone(), t.to.0.clone())))
            .collect();
        pairs.sort();
        let mut tested: Vec<(String, String)> = DEFAULT_RULES
            .iter()
            .map(|(from, to, _, _)| (from.to_string(), to.to_string()))
            .collect();
        tested.sort();
        tested.dedup();
        assert_eq!(tested, pairs);
    }

    #[test]
    fn refusal_messages_name_the_rule() {
        let workflow = default_workflow();
        let message = |from, to, role| {
            workflow
                .check_transition(from, to, role)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            message("start", "in_progress", Person),
            "only a runner makes start → in_progress, not a person"
        );
        assert_eq!(
            message("backlog", "done", Person),
            "no transition backlog → done exists in the workflow"
        );
        assert_eq!(
            message("merging", "done", Person),
            "merging → done happens only on the `pr_merged` event, not on request"
        );
        assert_eq!(
            message("human_review", "merging", Agent),
            "human_review is locked: an agent cannot act on an issue in it"
        );
        assert_eq!(
            message("backlog", "shipped", Person),
            "status `shipped` is not in the workflow"
        );
    }

    #[test]
    fn several_allowed_roles_are_listed() {
        let workflow = Workflow::from_toml(&format!(
            "statuses = [\"a\", \"b\", \"start\", \"in_progress\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\nby = [\"person\", \"agent\"]\n{RESERVED}"
        ))
        .unwrap();
        assert_eq!(
            workflow
                .check_transition("a", "b", Runner)
                .unwrap_err()
                .to_string(),
            "only a person or an agent makes a → b, not a runner"
        );
    }

    #[test]
    fn default_workflow_shape() {
        let workflow = default_workflow();
        assert_eq!(workflow.initial_status().as_str(), "backlog");
        assert_eq!(workflow.statuses().len(), 9);
        assert_eq!(workflow.transitions().len(), 8);
        assert!(workflow.version().starts_with("sha256:"));
        assert_eq!(workflow.version().len(), "sha256:".len() + 64);
    }

    #[test]
    fn default_dispatch_names_claude_code_for_in_progress() {
        let workflow = default_workflow();
        let rule = workflow.dispatch_for("in_progress").unwrap();
        assert_eq!(rule.agent, "claude-code");
        assert_eq!(rule.prompt.as_deref(), Some("prompts/in_progress.md"));
        assert_eq!(
            workflow.dispatch_for("agent_review").unwrap().agent,
            "codex"
        );
        assert_eq!(workflow.dispatch_for("start"), None);
    }

    #[test]
    fn events_move_only_from_their_from_statuses() {
        let workflow = default_workflow();
        let to = |from, event| {
            workflow
                .transition_on(from, event)
                .map(|t| (t.to.as_str().to_string(), t.require.clone()))
        };
        assert_eq!(to("merging", "pr_merged"), Some(("done".into(), vec![])));
        assert_eq!(to("human_review", "pr_merged"), None);
        assert_eq!(
            to("in_progress", "pr_closed_unmerged"),
            Some(("todo".into(), vec!["comment".into()]))
        );
        assert_eq!(
            to("agent_review", "pr_closed_unmerged"),
            Some(("todo".into(), vec!["comment".into()]))
        );
        assert_eq!(to("merging", "pr_closed_unmerged"), None);
        assert_eq!(to("todo", "pr_closed_unmerged"), None);
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("backlog"), "Backlog");
        assert_eq!(display_name("in_progress"), "In Progress");
        assert_eq!(display_name("human_review"), "Human Review");
    }

    /// The line, key and problem of a load error, for literal asserts.
    fn error(text: &str) -> (Option<usize>, String, Problem) {
        let e = Workflow::from_toml(text).unwrap_err();
        (e.line, e.key, e.problem)
    }

    /// What a claim needs, for fixtures that must load: add `start` and `in_progress` to
    /// `statuses` and append this.
    const RESERVED: &str = "[[transition]]\nfrom = \"start\"\nto = \"in_progress\"\nby = [\"runner\"]\n[dispatch.status.in_progress]\nagent = \"claude-code\"\n";

    const PRD_TEMPLATE: &str = include_str!("default/workflow.toml");

    #[test]
    fn the_prd_template_parses() {
        let workflow = Workflow::from_toml(PRD_TEMPLATE).unwrap();
        let statuses: Vec<&str> = workflow.statuses().iter().map(Status::as_str).collect();
        assert_eq!(
            statuses,
            [
                "backlog",
                "todo",
                "start",
                "in_progress",
                "agent_review",
                "human_review",
                "merging",
                "done",
                "canceled"
            ]
        );
        let transitions: Vec<String> = workflow
            .transitions()
            .iter()
            .map(|t| {
                let from: Vec<&str> = t.from.iter().map(Status::as_str).collect();
                let by: Vec<&str> = t.by.iter().map(|r| r.as_str()).collect();
                let gates: Vec<&str> = t.gates.iter().map(|g| g.as_str()).collect();
                format!(
                    "{} -> {} by [{}] on {} gates [{}] require [{}]",
                    from.join(","),
                    t.to,
                    by.join(","),
                    t.on.as_deref().unwrap_or("-"),
                    gates.join(","),
                    t.require.join(",")
                )
            })
            .collect();
        assert_eq!(
            transitions,
            [
                "backlog -> todo by [person] on - gates [] require []",
                "todo -> start by [person] on - gates [] require []",
                "start -> in_progress by [runner] on - gates [] require []",
                "in_progress -> agent_review by [agent] on - gates [pr_open,pr_ready,pr_names_issue] require []",
                "agent_review -> human_review by [agent] on - gates [ci_green,mergeable,no_open_threads,proof_attached] require []",
                "human_review -> merging by [person] on - gates [] require []",
                "merging -> done by [] on pr_merged gates [] require []",
                "in_progress,agent_review -> todo by [] on pr_closed_unmerged gates [] require [comment]",
            ]
        );
        let locks: Vec<(String, Vec<Role>)> = workflow
            .locks()
            .iter()
            .map(|(s, roles)| (s.to_string(), roles.clone()))
            .collect();
        assert_eq!(locks, [("human_review".to_string(), vec![Agent, Runner])]);
    }

    #[test]
    fn an_unknown_status_names_its_line_and_key() {
        let text = "statuses = [\"a\", \"b\"]\n\n[[transition]]\nfrom = \"a\"\nto = \"b\"\nby = [\"person\"]\n\n[[transition]]\nfrom = \"a\"\nto = \"z\"\nby = [\"person\"]\n";
        assert_eq!(
            error(text),
            (
                Some(10),
                "transition[1].to".into(),
                Problem::UnknownStatus("z".into())
            )
        );
        assert_eq!(
            Workflow::from_toml(text).unwrap_err().to_string(),
            "line 10, `transition[1].to`: status `z` is not in `statuses`"
        );
    }

    #[test]
    fn an_unknown_gate_names_its_line_and_key() {
        let text = PRD_TEMPLATE.replace(
            "gates = [\"pr_open\", \"pr_ready\", \"pr_names_issue\"]",
            "gates = [\"pr_open\", \"pr_redy\", \"pr_names_issue\"]",
        );
        assert_eq!(
            error(&text),
            (
                Some(26),
                "transition[3].gates[1]".into(),
                Problem::UnknownGate("pr_redy".into())
            )
        );
        assert_eq!(
            Workflow::from_toml(&text).unwrap_err().to_string(),
            "line 26, `transition[3].gates[1]`: unknown gate `pr_redy`; gates are pr_open, pr_ready, pr_names_issue, ci_green, mergeable, no_open_threads, proof_attached"
        );
    }

    #[test]
    fn a_transition_with_neither_by_nor_on_names_its_line_and_key() {
        let text = "statuses = [\"a\", \"b\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\n";
        assert_eq!(
            error(text),
            (Some(4), "transition[0]".into(), Problem::NoActor)
        );
    }

    #[test]
    fn other_load_errors() {
        assert_eq!(
            error("statuses = []"),
            (Some(1), "statuses".into(), Problem::NoStatuses)
        );
        assert_eq!(
            error("statuses = [\"a\", \"a\"]"),
            (
                Some(1),
                "statuses[1]".into(),
                Problem::DuplicateStatus("a".into())
            )
        );
        assert_eq!(
            error("version = 2\nstatuses = [\"a\"]"),
            (Some(1), "version".into(), Problem::UnsupportedVersion(2))
        );
        let transition = |extra: &str| {
            error(&format!(
                "statuses = [\"a\", \"b\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\n{extra}"
            ))
        };
        assert_eq!(
            transition("on = \"pr_merge\""),
            (
                Some(5),
                "transition[0].on".into(),
                Problem::UnknownEvent("pr_merge".into())
            )
        );
        assert_eq!(
            transition("by = [\"person\"]\nrequire = [\"evidence\"]"),
            (
                Some(6),
                "transition[0].require[0]".into(),
                Problem::UnknownRequirement("evidence".into())
            )
        );
        assert_eq!(
            transition("by = [\"bot\"]"),
            (
                Some(5),
                "transition[0].by[0]".into(),
                Problem::UnknownRole("bot".into())
            )
        );
        assert_eq!(
            transition("by = [\"github\"]").2,
            Problem::UnknownRole("github".into())
        );
        assert!(Workflow::from_toml(&format!("statuses = [\"a\", \"b\", \"start\", \"in_progress\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\non = \"pr_merged\"\nrequire = [\"comment\"]\n{RESERVED}"))
        .is_ok());
        assert_eq!(
            error(
                "statuses = [\"a\", \"b\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\nby = [\"person\"]\n[[transition]]\nfrom = [\"a\"]\nto = \"b\"\nby = [\"agent\"]"
            ),
            (
                Some(8),
                "transition[1]".into(),
                Problem::DuplicateTransition {
                    from: "a".into(),
                    to: "b".into()
                }
            )
        );
        assert_eq!(
            error("statuses = [\"a\"]\n[lock]\nz = [\"agent\"]"),
            (Some(3), "lock.z".into(), Problem::UnknownStatus("z".into()))
        );
        assert_eq!(
            error("statuses = [\"a\"]\n[dispatch.status.z]\nagent = \"codex\""),
            (
                Some(3),
                "dispatch.status.z".into(),
                Problem::UnknownStatus("z".into())
            )
        );
        assert_eq!(
            error("statuses = [\"a\"]\n[dispatch.status.a]\nagent = \"\""),
            (Some(3), "dispatch.status.a.agent".into(), Problem::NoAgent)
        );
        assert_eq!(
            error(
                "statuses = [\"a\"]\n[dispatch.status.a]\nagent = \"x\"\nprompt = \"../secrets\""
            ),
            (
                Some(4),
                "dispatch.status.a.prompt".into(),
                Problem::BadPromptPath("../secrets".into())
            )
        );
    }

    #[test]
    fn a_workflow_the_runtime_cannot_run_names_its_line_and_key() {
        let renamed = PRD_TEMPLATE.replace("\"start\"", "\"begin\"");
        assert_eq!(
            Workflow::from_toml(&renamed).unwrap_err().to_string(),
            "line 4, `statuses`: the runtime needs status `start` in `statuses`; `start`, `in_progress`, the runner transition between them and the `in_progress` dispatch rule are reserved"
        );
        let missing_status = PRD_TEMPLATE.replace("\"start\", ", "");
        let want = "line 14, `transition[1].to`: status `start` is not in `statuses`";
        assert_eq!(
            Workflow::from_toml(&missing_status)
                .unwrap_err()
                .to_string(),
            want
        );
        let no_rule = PRD_TEMPLATE.replace(
            "[dispatch.status.in_progress]\nagent = \"claude-code\"\nprompt = \"prompts/in_progress.md\"\n",
            "",
        );
        assert_eq!(
            Workflow::from_toml(&no_rule).unwrap_err().to_string(),
            "line 4, `dispatch.status.in_progress`: the runtime needs a `[dispatch.status.in_progress]` rule; `start`, `in_progress`, the runner transition between them and the `in_progress` dispatch rule are reserved"
        );
        let no_runner = PRD_TEMPLATE.replace("by = [\"runner\"]", "by = [\"person\"]");
        assert_eq!(
            Workflow::from_toml(&no_runner).unwrap_err().to_string(),
            "line 4, `transition`: the runtime needs a `[[transition]]` from `start` to `in_progress` with `by = [\"runner\"]`; `start`, `in_progress`, the runner transition between them and the `in_progress` dispatch rule are reserved"
        );
    }

    #[test]
    fn an_unknown_key_is_refused() {
        // A misspelt `gates` must not load as a transition with no gates.
        let (line, key, problem) = error(
            "statuses = [\"a\", \"b\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\"\nby = [\"agent\"]\ngate = [\"pr_open\"]",
        );
        assert_eq!((line, key), (Some(6), String::new()));
        assert!(matches!(problem, Problem::Toml(m) if m.starts_with("unknown field `gate`")));
    }

    #[test]
    fn prompts_load_hash_and_join() {
        let text = &format!(
            "statuses = [\"a\", \"start\", \"in_progress\"]\n[dispatch.status.a]\nagent = \"claude-code\"\nprompt = \"prompts/a.md\"\n{RESERVED}"
        );
        let files = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(p, t)| (p.to_string(), t.to_string()))
                .collect()
        };
        let missing = Workflow::from_files(text, files(&[])).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "line 4, `dispatch.status.a.prompt`: prompt file `.punchlist/prompts/a.md` does not exist"
        );
        let workflow = Workflow::from_files(
            text,
            files(&[
                ("prompts/a.md", "Do A.\n"),
                (SYSTEM_PROMPT, "You are an agent.\n"),
                ("prompts/unused.md", "x"),
            ]),
        )
        .unwrap();
        assert_eq!(workflow.prompt_for("a"), "You are an agent.\n\nDo A.");
        assert_eq!(
            workflow.prompts().keys().collect::<Vec<_>>(),
            ["prompts/a.md", "prompts/system.md"]
        );
        assert_eq!(
            workflow.prompt_paths(),
            ["prompts/system.md", "prompts/a.md"]
        );
        let edited = Workflow::from_files(text, files(&[("prompts/a.md", "Do B.\n")])).unwrap();
        assert_ne!(workflow.version(), edited.version());
        assert_ne!(
            workflow.version(),
            Workflow::from_toml(text).unwrap().version()
        );
    }

    #[test]
    fn punchlists_own_workflow_loads() {
        let prompts = [
            (
                "prompts/system.md",
                include_str!("../../../.punchlist/prompts/system.md"),
            ),
            (
                "prompts/in_progress.md",
                include_str!("../../../.punchlist/prompts/in_progress.md"),
            ),
            (
                "prompts/agent_review.md",
                include_str!("../../../.punchlist/prompts/agent_review.md"),
            ),
        ]
        .map(|(path, text)| (path.to_string(), text.to_string()));
        let workflow = Workflow::from_files(
            include_str!("../../../.punchlist/workflow.toml"),
            prompts.into_iter().collect(),
        )
        .unwrap();
        assert_eq!(workflow.statuses().len(), 9);
        assert_eq!(workflow.prompts().len(), 3);
    }

    #[test]
    fn the_schema_lists_the_gates() {
        let schema = serde_json::to_value(workflow_schema()).unwrap();
        let text = schema.to_string();
        for gate in Gate::ALL {
            assert!(
                text.contains(&format!("\"{gate}\"")),
                "{gate} in the schema"
            );
        }
        assert_eq!(schema["title"], "Punchlist workflow");
    }
}
