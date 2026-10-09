//! The workflow: statuses, transitions and who may make each one.
//!
//! Only the parts the server enforces are typed here. Gates are read but not yet evaluated,
//! so a gated transition is always refused. Dispatch rules name the agent a runner starts
//! for a status. Locks are read by later slices.

use std::fmt;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_WORKFLOW_TOML: &str = include_str!("default_workflow.toml");

static DEFAULT_WORKFLOW: LazyLock<Workflow> = LazyLock::new(|| {
    Workflow::from_toml(DEFAULT_WORKFLOW_TOML).expect("the built-in workflow is valid")
});

/// The built-in workflow a new workspace starts with (ADR 0010).
pub fn default_workflow() -> &'static Workflow {
    &DEFAULT_WORKFLOW
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
    pub gates: Vec<String>,
    /// What the move must carry, such as `comment`.
    pub require: Vec<String>,
}

/// Which agent a runner starts when an issue enters a status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    pub status: Status,
    /// An agent key, such as `claude-code`.
    pub agent: String,
    /// A path under `.punchlist/`. Read once the workflow file is loaded (Slice 4).
    pub prompt: Option<String>,
}

/// A parsed and validated workflow.
#[derive(Debug, Clone)]
pub struct Workflow {
    version: String,
    statuses: Vec<Status>,
    transitions: Vec<Transition>,
    dispatch: Vec<Dispatch>,
}

/// Why a workflow file did not load.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LoadError {
    #[error("{0}")]
    Toml(String),
    #[error("the workflow lists no statuses")]
    NoStatuses,
    #[error("status `{0}` is listed twice")]
    DuplicateStatus(String),
    #[error("a transition names status `{0}`, which is not in `statuses`")]
    UnknownStatus(String),
    #[error("transition to `{0}` has neither `by` nor `on`")]
    NoActor(String),
    #[error("dispatch rule for `{0}` names no agent")]
    NoAgent(String),
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
    /// Gates are not evaluated yet, so a gated move fails closed rather than skipping them.
    #[error("{from} → {to} needs gates {}, which Punchlist does not evaluate yet", gates.join(", "))]
    GatesNotEvaluated {
        from: String,
        to: String,
        gates: Vec<String>,
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
            Refusal::GatesNotEvaluated { .. } => "gates_not_evaluated",
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

impl Workflow {
    /// Parses and validates a `workflow.toml`.
    pub fn from_toml(text: &str) -> Result<Workflow, LoadError> {
        let raw: RawWorkflow = toml::from_str(text).map_err(|e| LoadError::Toml(e.to_string()))?;
        if raw.statuses.is_empty() {
            return Err(LoadError::NoStatuses);
        }
        let mut statuses: Vec<Status> = Vec::with_capacity(raw.statuses.len());
        for name in raw.statuses {
            if statuses.iter().any(|s| s.0 == name) {
                return Err(LoadError::DuplicateStatus(name));
            }
            statuses.push(Status(name));
        }
        let known = |name: String| -> Result<Status, LoadError> {
            if statuses.iter().any(|s| s.0 == name) {
                Ok(Status(name))
            } else {
                Err(LoadError::UnknownStatus(name))
            }
        };
        let mut transitions = Vec::with_capacity(raw.transitions.len());
        for t in raw.transitions {
            let from = match t.from {
                OneOrMany::One(name) => vec![known(name)?],
                OneOrMany::Many(names) => names.into_iter().map(known).collect::<Result<_, _>>()?,
            };
            let to = known(t.to)?;
            if t.by.is_empty() && t.on.is_none() {
                return Err(LoadError::NoActor(to.0));
            }
            transitions.push(Transition {
                from,
                to,
                by: t.by,
                on: t.on,
                gates: t.gates,
                require: t.require,
            });
        }
        let mut dispatch = Vec::with_capacity(raw.dispatch.status.len());
        for (name, rule) in raw.dispatch.status {
            let status = known(name)?;
            if rule.agent.trim().is_empty() {
                return Err(LoadError::NoAgent(status.0));
            }
            dispatch.push(Dispatch {
                status,
                agent: rule.agent,
                prompt: rule.prompt,
            });
        }
        Ok(Workflow {
            version: content_hash(text),
            statuses,
            transitions,
            dispatch,
        })
    }

    /// A content hash of the workflow file, recorded on every transition.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn statuses(&self) -> &[Status] {
        &self.statuses
    }

    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
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

    /// Applies the `by` rules: may `role` move an issue from `from` to `to`? A gated move is
    /// refused until gates are evaluated.
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
        let transition = self
            .transitions
            .iter()
            .find(|t| t.to.0 == to && t.from.iter().any(|f| f.0 == from))
            .ok_or_else(|| Refusal::NoTransition {
                from: from.to_string(),
                to: to.to_string(),
            })?;
        if transition.by.contains(&role) {
            if !transition.gates.is_empty() {
                return Err(Refusal::GatesNotEvaluated {
                    from: from.to_string(),
                    to: to.to_string(),
                    gates: transition.gates.clone(),
                });
            }
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

fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

#[derive(Deserialize)]
struct RawWorkflow {
    statuses: Vec<String>,
    #[serde(default, rename = "transition")]
    transitions: Vec<RawTransition>,
    #[serde(default)]
    dispatch: RawDispatch,
}

#[derive(Default, Deserialize)]
struct RawDispatch {
    #[serde(default)]
    status: std::collections::BTreeMap<String, RawDispatchRule>,
}

#[derive(Deserialize)]
struct RawDispatchRule {
    agent: String,
    prompt: Option<String>,
}

#[derive(Deserialize)]
struct RawTransition {
    from: OneOrMany,
    to: String,
    #[serde(default)]
    by: Vec<Role>,
    on: Option<String>,
    #[serde(default)]
    gates: Vec<String>,
    #[serde(default)]
    require: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    use Role::{Agent, Person, Runner};

    /// Every transition in the default workflow, crossed with every role. `Ok` means the
    /// role may make the move; otherwise the refusal code. Gated moves are refused for the
    /// roles in `by` too, until gates are evaluated.
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
        (
            "in_progress",
            "agent_review",
            Agent,
            Err("gates_not_evaluated"),
        ),
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
        (
            "agent_review",
            "human_review",
            Agent,
            Err("gates_not_evaluated"),
        ),
        (
            "agent_review",
            "human_review",
            Runner,
            Err("role_not_allowed"),
        ),
        ("human_review", "merging", Person, Ok(())),
        ("human_review", "merging", Agent, Err("role_not_allowed")),
        ("human_review", "merging", Runner, Err("role_not_allowed")),
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
            message("in_progress", "agent_review", Agent),
            "in_progress → agent_review needs gates pr_open, pr_ready, pr_names_issue, which Punchlist does not evaluate yet"
        );
        assert_eq!(
            message("backlog", "shipped", Person),
            "status `shipped` is not in the workflow"
        );
    }

    #[test]
    fn several_allowed_roles_are_listed() {
        let workflow = Workflow::from_toml(
            r#"
            statuses = ["a", "b"]
            [[transition]]
            from = "a"
            to = "b"
            by = ["person", "agent"]
            "#,
        )
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
    fn dispatch_errors() {
        assert_eq!(
            Workflow::from_toml("statuses = [\"a\"]\n[dispatch.status.z]\nagent = \"codex\"")
                .unwrap_err(),
            LoadError::UnknownStatus("z".into())
        );
        assert_eq!(
            Workflow::from_toml("statuses = [\"a\"]\n[dispatch.status.a]\nagent = \"\"")
                .unwrap_err(),
            LoadError::NoAgent("a".into())
        );
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

    #[test]
    fn load_errors() {
        assert_eq!(
            Workflow::from_toml("statuses = []").unwrap_err(),
            LoadError::NoStatuses
        );
        assert_eq!(
            Workflow::from_toml(r#"statuses = ["a", "a"]"#).unwrap_err(),
            LoadError::DuplicateStatus("a".into())
        );
        assert_eq!(
            Workflow::from_toml(
                "statuses = [\"a\"]\n[[transition]]\nfrom = \"a\"\nto = \"z\"\nby = [\"person\"]"
            )
            .unwrap_err(),
            LoadError::UnknownStatus("z".into())
        );
        assert_eq!(
            Workflow::from_toml(
                "statuses = [\"a\", \"b\"]\n[[transition]]\nfrom = \"a\"\nto = \"b\""
            )
            .unwrap_err(),
            LoadError::NoActor("b".into())
        );
    }
}
