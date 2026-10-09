//! Gates: checks a transition must pass first. Each gate is a pure function over recorded
//! evidence, so an agent cannot pass one by saying it passed.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::link::{branch_issue_id, title_issue_id};

/// The gates v1 knows (PRD R4). A workflow that names another gate does not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gate {
    /// The issue has exactly one open pull request.
    PrOpen,
    /// That pull request is not a draft.
    PrReady,
    /// Its branch or title names the issue, and neither names another issue.
    PrNamesIssue,
    CiGreen,
    Mergeable,
    NoOpenThreads,
    ProofAttached,
}

impl Gate {
    pub const ALL: [Gate; 7] = [
        Gate::PrOpen,
        Gate::PrReady,
        Gate::PrNamesIssue,
        Gate::CiGreen,
        Gate::Mergeable,
        Gate::NoOpenThreads,
        Gate::ProofAttached,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Gate::PrOpen => "pr_open",
            Gate::PrReady => "pr_ready",
            Gate::PrNamesIssue => "pr_names_issue",
            Gate::CiGreen => "ci_green",
            Gate::Mergeable => "mergeable",
            Gate::NoOpenThreads => "no_open_threads",
            Gate::ProofAttached => "proof_attached",
        }
    }

    pub fn parse(s: &str) -> Option<Gate> {
        Gate::ALL.into_iter().find(|gate| gate.as_str() == s)
    }

    /// Checks the gate against the evidence. The reason says why it passed or failed.
    pub fn evaluate(self, evidence: &Evidence) -> GateResult {
        let outcome = match self {
            Gate::PrOpen => {
                one_open(evidence).map(|pr| format!("pull request #{} is open", pr.number))
            }
            Gate::PrReady => one_open(evidence).and_then(|pr| {
                if pr.draft {
                    Err(format!("pull request #{} is a draft", pr.number))
                } else {
                    Ok(format!("pull request #{} is ready for review", pr.number))
                }
            }),
            Gate::PrNamesIssue => one_open(evidence).and_then(|pr| names_issue(evidence, pr)),
            // Evaluated from Slice 5. Until then a transition that names one fails closed.
            Gate::CiGreen | Gate::Mergeable | Gate::NoOpenThreads | Gate::ProofAttached => {
                Err(format!("Punchlist does not evaluate the {self} gate yet"))
            }
        };
        let (passed, reason) = match outcome {
            Ok(reason) => (true, reason),
            Err(reason) => (false, reason),
        };
        GateResult {
            gate: self.as_str().to_string(),
            passed,
            reason,
        }
    }
}

impl fmt::Display for Gate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A pull request's state on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

impl PullRequestState {
    pub fn as_str(self) -> &'static str {
        match self {
            PullRequestState::Open => "open",
            PullRequestState::Closed => "closed",
            PullRequestState::Merged => "merged",
        }
    }

    pub fn parse(s: &str) -> Option<PullRequestState> {
        [
            PullRequestState::Open,
            PullRequestState::Closed,
            PullRequestState::Merged,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }
}

/// A pull request linked to the issue, as GitHub's events recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PullRequestEvidence {
    pub number: i64,
    pub title: String,
    pub branch: String,
    pub state: PullRequestState,
    pub draft: bool,
}

/// What the gates read for one issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Evidence {
    /// Such as `PL-7`.
    pub issue_id: String,
    /// The pull requests linked to the issue, open or not.
    pub pull_requests: Vec<PullRequestEvidence>,
}

/// One gate's result. `reason` says why it passed or failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GateResult {
    pub gate: String,
    pub passed: bool,
    pub reason: String,
}

/// The issue's one open pull request. One issue has one pull request (PRD principle 1).
fn one_open(evidence: &Evidence) -> Result<&PullRequestEvidence, String> {
    let open: Vec<&PullRequestEvidence> = evidence
        .pull_requests
        .iter()
        .filter(|pr| pr.state == PullRequestState::Open)
        .collect();
    match open.as_slice() {
        [] => Err(format!(
            "no open pull request is linked to {}",
            evidence.issue_id
        )),
        [pr] => Ok(pr),
        many => Err(format!(
            "{} has {} open pull requests ({}); an issue has one",
            evidence.issue_id,
            many.len(),
            many.iter()
                .map(|pr| format!("#{}", pr.number))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn names_issue(evidence: &Evidence, pr: &PullRequestEvidence) -> Result<String, String> {
    let issue = evidence.issue_id.as_str();
    let prefix = issue.rsplit_once('-').map_or(issue, |(prefix, _)| prefix);
    let branch = branch_issue_id(prefix, &pr.branch);
    let title = title_issue_id(prefix, &pr.title);
    for (part, named) in [("branch", &branch), ("title", &title)] {
        if let Some(other) = named.as_deref().filter(|named| *named != issue) {
            return Err(format!(
                "pull request #{}'s {part} names {other}, not {issue}",
                pr.number
            ));
        }
    }
    match (branch.is_some(), title.is_some()) {
        (true, true) => Ok(format!(
            "pull request #{}'s branch and title name {issue}",
            pr.number
        )),
        (true, false) => Ok(format!(
            "pull request #{}'s branch names {issue}",
            pr.number
        )),
        (false, true) => Ok(format!("pull request #{}'s title names {issue}", pr.number)),
        (false, false) => Err(format!(
            "pull request #{}'s branch and title name no issue; end the title with ({issue})",
            pr.number
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(
        number: i64,
        branch: &str,
        title: &str,
        state: PullRequestState,
        draft: bool,
    ) -> PullRequestEvidence {
        PullRequestEvidence {
            number,
            title: title.into(),
            branch: branch.into(),
            state,
            draft,
        }
    }

    fn evidence(pull_requests: Vec<PullRequestEvidence>) -> Evidence {
        Evidence {
            issue_id: "PL-7".into(),
            pull_requests,
        }
    }

    fn check(gate: Gate, evidence: &Evidence) -> (bool, String) {
        let result = gate.evaluate(evidence);
        assert_eq!(result.gate, gate.as_str());
        (result.passed, result.reason)
    }

    const OPEN: PullRequestState = PullRequestState::Open;

    #[test]
    fn no_pull_request_fails_all_three() {
        let none = evidence(vec![pr(
            3,
            "feature/pl-7-x",
            "X",
            PullRequestState::Closed,
            false,
        )]);
        for gate in [Gate::PrOpen, Gate::PrReady, Gate::PrNamesIssue] {
            assert_eq!(
                check(gate, &none),
                (false, "no open pull request is linked to PL-7".into())
            );
        }
    }

    #[test]
    fn a_draft_is_open_but_not_ready() {
        let draft = evidence(vec![pr(12, "feature/pl-7-fix", "Fix (PL-7)", OPEN, true)]);
        assert_eq!(
            check(Gate::PrOpen, &draft),
            (true, "pull request #12 is open".into())
        );
        assert_eq!(
            check(Gate::PrReady, &draft),
            (false, "pull request #12 is a draft".into())
        );
        assert_eq!(
            check(Gate::PrNamesIssue, &draft),
            (true, "pull request #12's branch and title name PL-7".into())
        );
    }

    #[test]
    fn a_ready_pull_request_passes() {
        let ready = evidence(vec![
            pr(
                9,
                "feature/pl-7-old",
                "Old (PL-7)",
                PullRequestState::Closed,
                false,
            ),
            pr(12, "feature/pl-7-fix", "Fix", OPEN, false),
        ]);
        assert_eq!(
            check(Gate::PrReady, &ready),
            (true, "pull request #12 is ready for review".into())
        );
        assert_eq!(
            check(Gate::PrNamesIssue, &ready),
            (true, "pull request #12's branch names PL-7".into())
        );
    }

    #[test]
    fn a_title_naming_another_issue_fails() {
        let other = evidence(vec![pr(12, "feature/pl-7-fix", "Fix (PL-9)", OPEN, false)]);
        assert_eq!(
            check(Gate::PrNamesIssue, &other),
            (
                false,
                "pull request #12's title names PL-9, not PL-7".into()
            )
        );
        assert!(check(Gate::PrOpen, &other).0);
    }

    #[test]
    fn a_branch_naming_another_issue_fails() {
        let other = evidence(vec![pr(12, "feature/pl-9-fix", "Fix (PL-7)", OPEN, false)]);
        assert_eq!(
            check(Gate::PrNamesIssue, &other),
            (
                false,
                "pull request #12's branch names PL-9, not PL-7".into()
            )
        );
    }

    #[test]
    fn the_title_alone_names_the_issue() {
        let titled = evidence(vec![pr(12, "fix-it", "Fix it (pl-7)", OPEN, false)]);
        assert_eq!(
            check(Gate::PrNamesIssue, &titled),
            (true, "pull request #12's title names PL-7".into())
        );
        let unnamed = evidence(vec![pr(12, "fix-it", "Fix it", OPEN, false)]);
        assert_eq!(
            check(Gate::PrNamesIssue, &unnamed),
            (
                false,
                "pull request #12's branch and title name no issue; end the title with (PL-7)"
                    .into()
            )
        );
    }

    #[test]
    fn two_open_pull_requests_fail() {
        let two = evidence(vec![
            pr(13, "feature/pl-7-b", "B", OPEN, false),
            pr(12, "feature/pl-7-a", "A", OPEN, false),
        ]);
        assert_eq!(
            check(Gate::PrOpen, &two),
            (
                false,
                "PL-7 has 2 open pull requests (#13, #12); an issue has one".into()
            )
        );
    }

    #[test]
    fn later_gates_fail_closed() {
        let ready = evidence(vec![pr(12, "feature/pl-7-fix", "Fix", OPEN, false)]);
        assert_eq!(
            check(Gate::CiGreen, &ready),
            (
                false,
                "Punchlist does not evaluate the ci_green gate yet".into()
            )
        );
    }

    #[test]
    fn gate_names_round_trip() {
        for gate in Gate::ALL {
            assert_eq!(Gate::parse(gate.as_str()), Some(gate));
        }
        assert_eq!(Gate::parse("pr_opened"), None);
    }
}
