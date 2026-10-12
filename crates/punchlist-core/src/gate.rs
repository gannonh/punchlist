//! Gates: checks a transition must pass first. Each gate is a pure function over recorded
//! evidence, so an agent cannot pass one by saying it passed.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::link::{branch_issue_id, title_issue_id, title_issue_ids};

/// The gates v1 knows (PRD R4). A workflow that names another gate does not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gate {
    /// The issue has exactly one open pull request.
    PrOpen,
    /// That pull request is not a draft.
    PrReady,
    /// Its branch or title names the issue, and neither names another issue. On a move that
    /// a merged or closed pull request causes, "it" is that pull request.
    PrNamesIssue,
    /// Every check run on that pull request's head commit completed successfully, and each
    /// check the workflow requires has a run there from its app.
    CiGreen,
    /// GitHub's merge state for it is `clean` or `has_hooks`.
    Mergeable,
    /// None of its review threads is unresolved.
    NoOpenThreads,
    /// Passes without evidence until proof exists (Slice 7), and says so in its reason.
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
    /// `required` is the workflow's `[gate.ci_green]` list; only `ci_green` reads it.
    pub fn evaluate(self, evidence: &Evidence, required: &[RequiredCheck]) -> GateResult {
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
            Gate::PrNamesIssue => named(evidence).and_then(|pr| names_issue(evidence, pr)),
            Gate::CiGreen => one_open(evidence).and_then(|pr| ci_green(pr, required)),
            Gate::Mergeable => one_open(evidence).and_then(mergeable),
            Gate::NoOpenThreads => one_open(evidence).and_then(no_open_threads),
            Gate::ProofAttached => Ok(
                "passes vacuously: Punchlist records no proof yet, so nothing was checked".into(),
            ),
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
    /// The head commit.
    pub head_sha: String,
    /// GitHub's `mergeable_state`, such as `clean` or `dirty`; `unknown` until GitHub has
    /// computed it.
    pub merge_state: String,
    /// The check runs on the head commit, the newest of each name. Read for an open pull
    /// request.
    pub check_runs: Vec<CheckRunEvidence>,
    /// Its review threads, resolved or not. Read for an open pull request.
    pub review_threads: Vec<ReviewThreadEvidence>,
}

/// A check run on a pull request's head commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckRunEvidence {
    pub name: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// Set once completed, such as `success` or `failure`.
    pub conclusion: Option<String>,
    /// The slug of the GitHub App that created the run, such as `github-actions`; absent for a
    /// run recorded before apps were.
    #[serde(default)]
    pub app: Option<String>,
}

/// A check `ci_green` requires on the head commit: its name and the GitHub App that must have
/// produced it. From the workflow file's `[gate.ci_green]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredCheck {
    pub name: String,
    /// The app's slug, such as `github-actions`.
    pub app: String,
}

impl CheckRunEvidence {
    /// Whether the run passed: completed with success, neutral or skipped. `None` while it
    /// has not completed.
    pub fn passed(&self) -> Option<bool> {
        let passed = matches!(
            self.conclusion.as_deref(),
            Some("success" | "neutral" | "skipped")
        );
        (self.status == "completed").then_some(passed)
    }
}

/// A review thread on a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewThreadEvidence {
    /// The file it is on; absent for a thread recorded before paths were.
    pub path: Option<String>,
    /// The line it is on, or was on before the file changed under it.
    pub line: Option<i64>,
    pub resolved: bool,
}

/// What the gates read for one issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Evidence {
    /// Such as `PL-7`.
    pub issue_id: String,
    /// The pull requests linked to the issue, open or not.
    pub pull_requests: Vec<PullRequestEvidence>,
    /// The pull request that a GitHub event merged or closed without merging, when that event
    /// is causing the move; absent on a requested move, and read only by `pr_names_issue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_pull_request: Option<i64>,
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
        [] => {
            // Name the ones that are gone: a request made on a pull request that was merged
            // or closed should say so.
            let gone: Vec<String> = evidence
                .pull_requests
                .iter()
                .map(|pr| format!("pull request #{} is {}", pr.number, pr.state.as_str()))
                .collect();
            let none = format!("no open pull request is linked to {}", evidence.issue_id);
            Err(if gone.is_empty() {
                none
            } else {
                format!("{none}: {}", gone.join(", "))
            })
        }
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

/// The pull request `pr_names_issue` checks: the one a GitHub event merged or closed, or on
/// a requested move the issue's one open pull request.
fn named(evidence: &Evidence) -> Result<&PullRequestEvidence, String> {
    let Some(number) = evidence.closed_pull_request else {
        return one_open(evidence);
    };
    (evidence.pull_requests.iter())
        .find(|pr| pr.number == number)
        .ok_or_else(|| {
            format!(
                "pull request #{number} is not linked to {}",
                evidence.issue_id
            )
        })
}

fn names_issue(evidence: &Evidence, pr: &PullRequestEvidence) -> Result<String, String> {
    let issue = evidence.issue_id.as_str();
    let prefix = issue.rsplit_once('-').map_or(issue, |(prefix, _)| prefix);
    let branch = branch_issue_id(prefix, &pr.branch);
    let title = title_issue_id(prefix, &pr.title);
    let named = (branch.iter().cloned().map(|id| ("branch", id)))
        .chain(title_issue_ids(prefix, &pr.title).map(|id| ("title", id)));
    for (part, other) in named {
        if other != issue {
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

/// Text in backticks for a reason. A check's name and an app's slug are their author's text:
/// each is cut to 80 characters, without backticks or control characters, so it cannot close
/// the quotes.
fn quoted(text: &str) -> String {
    let text: String = (text.chars())
        .filter(|c| *c != '`' && !c.is_control())
        .take(80)
        .collect();
    format!("`{text}`")
}

/// At most five of `items`, then how many more, so a list cannot flood the reason.
fn capped(items: Vec<String>) -> String {
    const NAMED: usize = 5;
    let more = items.len().saturating_sub(NAMED);
    let mut named: Vec<String> = items.into_iter().take(NAMED).collect();
    if more > 0 {
        named.push(format!("and {more} more"));
    }
    named.join(", ")
}

/// Check runs named with their status or conclusion: `` `e2e` (failure) ``.
fn named_runs<'a>(runs: impl Iterator<Item = &'a CheckRunEvidence>) -> String {
    capped(
        runs.map(|run| {
            let state = run.conclusion.as_deref().unwrap_or(&run.status);
            format!("{} ({state})", quoted(&run.name))
        })
        .collect(),
    )
}

/// A required check with no run from its app on the commit, and where the runs of its name
/// did come from: `` `deploy` from `vercel` (ran from `github-actions`) ``. `None` when its
/// app has a run; whether that run passed is `ci_green`'s rule for every run.
fn missing(check: &RequiredCheck, runs: &[CheckRunEvidence]) -> Option<String> {
    let mut apps: Vec<Option<&str>> = Vec::new();
    for run in runs.iter().filter(|run| run.name == check.name) {
        if run.app.as_deref() == Some(check.app.as_str()) {
            return None;
        }
        if !apps.contains(&run.app.as_deref()) {
            apps.push(run.app.as_deref());
        }
    }
    let ran: Vec<String> = (apps.iter().flatten().map(|app| quoted(app))).collect();
    let mut found = Vec::new();
    if !ran.is_empty() {
        found.push(format!("ran from {}", capped(ran)));
    }
    // A run recorded before apps were has none, and so satisfies no required check.
    if apps.contains(&None) {
        found.push("a run of that name has no recorded app".to_string());
    }
    if found.is_empty() {
        found.push("no run".to_string());
    }
    Some(format!(
        "{} from {} ({})",
        quoted(&check.name),
        quoted(&check.app),
        found.join("; ")
    ))
}

/// Every check run on the head commit completed with success, neutral or skipped, and each
/// required check has a run there from its app. A commit with no check run fails: right after
/// a push GitHub has not started them, and passing then would let the request through before
/// CI ran. `pr.check_runs` is the newest run of each name in each suite, so two workflows with
/// a job of one name are two runs: a required check needs one of them from its app, and the
/// other still has to pass, as every run does.
fn ci_green(pr: &PullRequestEvidence, required: &[RequiredCheck]) -> Result<String, String> {
    // The first seven characters of the commit, as people quote one.
    let commit = pr.head_sha.get(..7).unwrap_or(&pr.head_sha);
    let total = pr.check_runs.len();
    let (mut failed, mut pending) = (Vec::new(), Vec::new());
    for run in &pr.check_runs {
        match run.passed() {
            Some(true) => {}
            Some(false) => failed.push(run),
            None => pending.push(run),
        }
    }
    // First, so that the reason names a required check that is not there whatever else ran.
    let absent: Vec<String> = (required.iter())
        .filter_map(|check| missing(check, &pr.check_runs))
        .collect();
    if !absent.is_empty() {
        Err(format!(
            "{} of {} required checks missing on {commit}: {}",
            absent.len(),
            required.len(),
            capped(absent)
        ))
    } else if total == 0 {
        Err(format!("no check run has reported on commit {commit}"))
    } else if !failed.is_empty() {
        Err(format!(
            "{} of {total} checks failed on {commit}: {}",
            failed.len(),
            named_runs(failed.into_iter())
        ))
    } else if !pending.is_empty() {
        Err(format!(
            "{} of {total} checks pending on {commit}: {}",
            pending.len(),
            named_runs(pending.into_iter())
        ))
    } else if required.is_empty() {
        Ok(format!("{total} of {total} checks passed on {commit}"))
    } else {
        Ok(format!(
            "{total} of {total} checks passed on {commit}, the {} required among them",
            required.len()
        ))
    }
}

/// GitHub's merge state is `clean`, or `has_hooks`. `unknown` is GitHub still computing it, which it starts
/// when the pull request is read, so the reason says to ask again.
fn mergeable(pr: &PullRequestEvidence) -> Result<String, String> {
    let number = pr.number;
    match pr.merge_state.as_str() {
        // `has_hooks` is GitHub's `clean` on a repository with pre-receive hooks.
        state @ ("clean" | "has_hooks") => Ok(format!(
            "pull request #{number} has no conflicts and GitHub's merge state is {state}"
        )),
        "unknown" | "" => Err(format!(
            "GitHub has not computed pull request #{number}'s merge state yet; request again"
        )),
        "dirty" => Err(format!(
            "pull request #{number} conflicts with its base branch (GitHub's merge state is dirty)"
        )),
        other => Err(format!(
            "GitHub's merge state for pull request #{number} is {other}, not clean"
        )),
    }
}

fn no_open_threads(pr: &PullRequestEvidence) -> Result<String, String> {
    let open: Vec<String> = (pr.review_threads.iter())
        .filter(|thread| !thread.resolved)
        .map(|thread| match (&thread.path, thread.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => "a thread with no recorded path".to_string(),
        })
        .collect();
    let threads = |n: usize| if n == 1 { "thread" } else { "threads" };
    match (open.len(), pr.review_threads.len()) {
        (0, 0) => Ok(format!("pull request #{} has no review threads", pr.number)),
        (0, all) => Ok(format!("{all} review {}, all resolved", threads(all))),
        (n, _) => Err(format!(
            "{n} unresolved review {} on pull request #{}: {}",
            threads(n),
            pr.number,
            open.join(", ")
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
            head_sha: "3f2a1c4d5e6f708192a3b4c5d6e7f8091a2b3c4d".into(),
            merge_state: "clean".into(),
            check_runs: Vec::new(),
            review_threads: Vec::new(),
        }
    }

    fn evidence(pull_requests: Vec<PullRequestEvidence>) -> Evidence {
        Evidence {
            issue_id: "PL-7".into(),
            pull_requests,
            closed_pull_request: None,
        }
    }

    fn check(gate: Gate, evidence: &Evidence) -> (bool, String) {
        let result = gate.evaluate(evidence, &[]);
        assert_eq!(result.gate, gate.as_str());
        (result.passed, result.reason)
    }

    const OPEN: PullRequestState = PullRequestState::Open;

    #[test]
    fn no_open_pull_request_fails_all_three_and_names_the_ones_that_are_gone() {
        let gone = evidence(vec![
            pr(
                12,
                "feature/pl-7-fix",
                "Fix",
                PullRequestState::Merged,
                false,
            ),
            pr(3, "feature/pl-7-x", "X", PullRequestState::Closed, false),
        ]);
        for gate in [Gate::PrOpen, Gate::PrReady, Gate::PrNamesIssue] {
            assert_eq!(
                check(gate, &evidence(vec![])),
                (false, "no open pull request is linked to PL-7".into())
            );
            assert_eq!(
                check(gate, &gone),
                (
                    false,
                    "no open pull request is linked to PL-7: pull request #12 is merged, \
                     pull request #3 is closed"
                        .into()
                )
            );
        }
    }

    #[test]
    fn the_pull_request_an_event_closed_can_name_the_issue() {
        let merged = Evidence {
            closed_pull_request: Some(12),
            ..evidence(vec![
                pr(13, "feature/pl-7-next", "Next", OPEN, false),
                pr(
                    12,
                    "feature/pl-7-fix",
                    "Fix",
                    PullRequestState::Merged,
                    false,
                ),
            ])
        };
        assert_eq!(
            check(Gate::PrNamesIssue, &merged),
            (true, "pull request #12's branch names PL-7".into())
        );
        // The gates about an open pull request still read the open one.
        assert_eq!(
            check(Gate::PrOpen, &merged),
            (true, "pull request #13 is open".into())
        );
        let other = Evidence {
            closed_pull_request: Some(12),
            ..evidence(vec![pr(
                12,
                "feature/pl-7-fix",
                "Fix (PL-9)",
                PullRequestState::Merged,
                false,
            )])
        };
        assert_eq!(
            check(Gate::PrNamesIssue, &other),
            (
                false,
                "pull request #12's title names PL-9, not PL-7".into()
            )
        );
        assert_eq!(
            check(Gate::PrOpen, &other),
            (
                false,
                "no open pull request is linked to PL-7: pull request #12 is merged".into()
            )
        );
        // An open pull request does not stand in for the one the event closed.
        let unlinked = Evidence {
            closed_pull_request: Some(12),
            ..evidence(vec![pr(13, "feature/pl-7-next", "Next", OPEN, false)])
        };
        assert_eq!(
            check(Gate::PrNamesIssue, &unlinked),
            (false, "pull request #12 is not linked to PL-7".into())
        );
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
        // The second id in a title counts too, with or without a branch that names the issue.
        for branch in ["feature/pl-7-fix", "fix"] {
            let second = evidence(vec![pr(
                12,
                branch,
                "Fix (PL-7), follow-up to (PL-9)",
                OPEN,
                false,
            )]);
            assert_eq!(
                check(Gate::PrNamesIssue, &second),
                (
                    false,
                    "pull request #12's title names PL-9, not PL-7".into()
                )
            );
        }
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

    /// An open, ready pull request #12 with these check runs, merge state and threads.
    fn reviewed(
        runs: &[(&str, &str, Option<&str>)],
        merge_state: &str,
        threads: &[(&str, Option<i64>, bool)],
    ) -> Evidence {
        let mut pr = pr(12, "feature/pl-7-fix", "Fix", OPEN, false);
        pr.merge_state = merge_state.into();
        pr.check_runs = (runs.iter())
            .map(|&(name, status, conclusion)| CheckRunEvidence {
                name: name.into(),
                status: status.into(),
                conclusion: conclusion.map(String::from),
                app: Some("github-actions".into()),
            })
            .collect();
        pr.review_threads = (threads.iter())
            .map(|&(path, line, resolved)| ReviewThreadEvidence {
                path: Some(path.into()),
                line,
                resolved,
            })
            .collect();
        evidence(vec![pr])
    }

    const PASSED: (&str, &str, Option<&str>) = ("test", "completed", Some("success"));

    #[test]
    fn a_check_name_cannot_close_the_quotes_or_flood_the_reason() {
        let long = "x".repeat(100);
        let runs: Vec<(&str, &str, Option<&str>)> = vec![
            ("a`b\nc", "completed", Some("failure")),
            (&long, "completed", Some("failure")),
            ("c", "completed", Some("failure")),
            ("d", "completed", Some("failure")),
            ("e", "completed", Some("failure")),
            ("f", "completed", Some("failure")),
            ("g", "completed", Some("failure")),
        ];
        let (passed, reason) = check(Gate::CiGreen, &reviewed(&runs, "clean", &[]));
        assert!(!passed);
        assert_eq!(
            reason,
            format!(
                "7 of 7 checks failed on 3f2a1c4: `abc` (failure), `{}` (failure), \
                 `c` (failure), `d` (failure), `e` (failure), and 2 more",
                "x".repeat(80)
            )
        );
    }

    #[test]
    fn ci_green_needs_every_check_run_on_the_head_commit_to_pass() {
        let skipped = ("docs", "completed", Some("skipped"));
        assert_eq!(
            check(Gate::CiGreen, &reviewed(&[PASSED, skipped], "clean", &[])),
            (true, "2 of 2 checks passed on 3f2a1c4".into())
        );
        let failed = ("e2e", "completed", Some("failure"));
        let timed_out = ("lint", "completed", Some("timed_out"));
        let running = ("build", "in_progress", None);
        assert_eq!(
            check(
                Gate::CiGreen,
                &reviewed(&[PASSED, failed, running, timed_out], "clean", &[])
            ),
            (
                false,
                "2 of 4 checks failed on 3f2a1c4: `e2e` (failure), `lint` (timed_out)".into()
            )
        );
    }

    #[test]
    fn ci_green_fails_while_a_check_run_is_pending_or_none_has_reported() {
        let running = ("e2e", "in_progress", None);
        let queued = ("lint", "queued", None);
        assert_eq!(
            check(
                Gate::CiGreen,
                &reviewed(&[PASSED, running, queued], "clean", &[])
            ),
            (
                false,
                "2 of 3 checks pending on 3f2a1c4: `e2e` (in_progress), `lint` (queued)".into()
            )
        );
        assert_eq!(
            check(Gate::CiGreen, &reviewed(&[], "clean", &[])),
            (false, "no check run has reported on commit 3f2a1c4".into())
        );
    }

    #[test]
    fn ci_green_needs_each_required_check_from_its_app() {
        let required = |name: &str, app: &str| RequiredCheck {
            name: name.into(),
            app: app.into(),
        };
        let ci = |runs: &[(&str, &str, Option<&str>)], required: &[RequiredCheck]| {
            let result = Gate::CiGreen.evaluate(&reviewed(runs, "clean", &[]), required);
            (result.passed, result.reason)
        };
        let test = [required("test", "github-actions")];
        // Present and passing.
        assert_eq!(
            ci(&[PASSED, ("lint", "completed", Some("success"))], &test),
            (
                true,
                "2 of 2 checks passed on 3f2a1c4, the 1 required among them".into()
            )
        );
        // Missing: the checks that ran all passed, and on a commit with none.
        let lint = ("lint", "completed", Some("success"));
        let no_run = "1 of 1 required checks missing on 3f2a1c4: \
                      `test` from `github-actions` (no run)";
        assert_eq!(ci(&[lint], &test), (false, no_run.into()));
        assert_eq!(ci(&[], &test), (false, no_run.into()));
        // Present from another app: the fixture's runs are GitHub Actions'.
        assert_eq!(
            ci(
                &[PASSED],
                &[required("test", "vercel"), required("lint", "vercel")]
            ),
            (
                false,
                "2 of 2 required checks missing on 3f2a1c4: `test` from `vercel` \
                 (ran from `github-actions`), `lint` from `vercel` (no run)"
                    .into()
            )
        );
        // A run of the name that failed or is pending is there: the rule for every run says so.
        assert_eq!(
            ci(&[("test", "completed", Some("failure"))], &test),
            (
                false,
                "1 of 1 checks failed on 3f2a1c4: `test` (failure)".into()
            )
        );
        assert_eq!(
            ci(&[("test", "queued", None)], &test),
            (
                false,
                "1 of 1 checks pending on 3f2a1c4: `test` (queued)".into()
            )
        );
        // Two workflows with a job of one name: one passing satisfies the requirement, and the
        // other still fails the gate.
        assert_eq!(
            ci(&[PASSED, ("test", "completed", Some("failure"))], &test),
            (
                false,
                "1 of 2 checks failed on 3f2a1c4: `test` (failure)".into()
            )
        );
    }

    #[test]
    fn a_run_with_no_recorded_app_or_a_hostile_slug_satisfies_no_required_check() {
        let mut evidence = reviewed(&[PASSED, PASSED], "clean", &[]);
        let runs = &mut evidence.pull_requests[0].check_runs;
        runs[0].app = None;
        runs[1].app = Some(format!("a`b\n{}", "x".repeat(100)));
        let required = [RequiredCheck {
            name: "test".into(),
            app: "github-actions".into(),
        }];
        let result = Gate::CiGreen.evaluate(&evidence, &required);
        assert!(!result.passed);
        assert_eq!(
            result.reason,
            format!(
                "1 of 1 required checks missing on 3f2a1c4: `test` from `github-actions` \
                 (ran from `ab{}`; a run of that name has no recorded app)",
                "x".repeat(78)
            )
        );
        // With no names set the app is not read.
        assert_eq!(
            check(Gate::CiGreen, &evidence),
            (true, "2 of 2 checks passed on 3f2a1c4".into())
        );
    }

    #[test]
    fn mergeable_needs_a_clean_merge_state() {
        let state = |merge_state| check(Gate::Mergeable, &reviewed(&[PASSED], merge_state, &[]));
        assert_eq!(
            state("clean"),
            (
                true,
                "pull request #12 has no conflicts and GitHub's merge state is clean".into()
            )
        );
        assert_eq!(
            state("has_hooks"),
            (
                true,
                "pull request #12 has no conflicts and GitHub's merge state is has_hooks".into()
            )
        );
        assert_eq!(
            state("dirty"),
            (
                false,
                "pull request #12 conflicts with its base branch (GitHub's merge state is dirty)"
                    .into()
            )
        );
        assert_eq!(
            state("unknown"),
            (
                false,
                "GitHub has not computed pull request #12's merge state yet; request again".into()
            )
        );
        assert_eq!(
            state("blocked"),
            (
                false,
                "GitHub's merge state for pull request #12 is blocked, not clean".into()
            )
        );
    }

    #[test]
    fn no_open_threads_names_each_unresolved_thread() {
        fn threads(threads: &[(&str, Option<i64>, bool)]) -> (bool, String) {
            check(Gate::NoOpenThreads, &reviewed(&[PASSED], "clean", threads))
        }
        assert_eq!(
            threads(&[]),
            (true, "pull request #12 has no review threads".into())
        );
        let resolved = ("src/gate.rs", Some(30), true);
        assert_eq!(
            threads(&[resolved]),
            (true, "1 review thread, all resolved".into())
        );
        // Resolved, then reopened: what counts is how the thread stands now.
        let reopened = ("src/gate.rs", Some(30), false);
        assert_eq!(
            threads(&[reopened]),
            (
                false,
                "1 unresolved review thread on pull request #12: src/gate.rs:30".into()
            )
        );
        let outdated = ("README.md", None, false);
        assert_eq!(
            threads(&[resolved, reopened, outdated]),
            (
                false,
                "2 unresolved review threads on pull request #12: src/gate.rs:30, README.md".into()
            )
        );
    }

    #[test]
    fn the_review_gates_need_the_one_open_pull_request() {
        for gate in [Gate::CiGreen, Gate::Mergeable, Gate::NoOpenThreads] {
            assert_eq!(
                check(gate, &evidence(vec![])),
                (false, "no open pull request is linked to PL-7".into())
            );
        }
    }

    #[test]
    fn proof_attached_passes_and_says_nothing_was_checked() {
        assert_eq!(
            check(Gate::ProofAttached, &evidence(vec![])),
            (
                true,
                "passes vacuously: Punchlist records no proof yet, so nothing was checked".into()
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
