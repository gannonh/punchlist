// Mock data for the issue page prototype. Nothing here is wired to production code.
// The workflow mirrors the example `workflow.toml` in docs/product/prd.md and the
// status definitions in prototypes/issue-list/data.js.

window.PUNCHLIST_PAGE_MOCK = (function () {
  const statuses = [
    { id: "backlog", label: "Backlog", kind: "backlog", next: { to: "todo", by: "person", gates: [] } },
    { id: "todo", label: "Todo", kind: "unstarted", next: { to: "start", by: "person", gates: [] } },
    { id: "start", label: "Start", kind: "unstarted", next: { to: "in_progress", by: "runner", gates: [] } },
    { id: "in_progress", label: "In Progress", kind: "started", dispatch: "claude-code", next: { to: "agent_review", by: "agent", gates: ["pr_open", "pr_ready", "pr_names_issue"] } },
    { id: "agent_review", label: "Agent Review", kind: "started", dispatch: "codex", next: { to: "human_review", by: "agent", gates: ["ci_green", "mergeable", "no_open_threads", "proof_attached"] } },
    { id: "human_review", label: "Human Review", kind: "started", lock: ["agent", "runner"], next: { to: "merging", by: "person", gates: [] } },
    { id: "merging", label: "Merging", kind: "started", next: { to: "done", by: "event", event: "pr_merged", gates: [] } },
    { id: "done", label: "Done", kind: "completed", next: null },
    { id: "canceled", label: "Canceled", kind: "canceled", next: null },
  ];

  // Each gate names the evidence it reads. `evidence` is an in-page anchor in the prototype.
  const gates = {
    pr_open: { label: "PR open", reads: "pull request", evidence: "pr" },
    pr_ready: { label: "PR ready", reads: "pull request", evidence: "pr" },
    pr_names_issue: { label: "PR names issue", reads: "pull request", evidence: "pr" },
    ci_green: { label: "CI green", reads: "check runs", evidence: "checks" },
    mergeable: { label: "Mergeable", reads: "merge state", evidence: "pr" },
    no_open_threads: { label: "No open threads", reads: "review threads", evidence: "threads" },
    proof_attached: { label: "Proof attached", reads: "proof", evidence: "proof" },
  };

  const actors = {
    gannon: { id: "gannon", type: "person", name: "Gannon Hall", initials: "GH" },
    priya: { id: "priya", type: "person", name: "Priya Natarajan", initials: "PN" },
    claude: { id: "claude", type: "agent", kind: "claude-code", name: "Claude Code", model: "claude-sonnet-5-5", initials: "CC" },
    codex: { id: "codex", type: "agent", kind: "codex", name: "Codex", model: "gpt-5-codex", initials: "CX" },
    opencode: { id: "opencode", type: "agent", kind: "opencode", name: "OpenCode", model: "qwen3-coder", initials: "OC" },
    sartre: { id: "sartre", type: "runner", name: "sartre", initials: "RN" },
    mini: { id: "mini", type: "runner", name: "mini", initials: "RN" },
    github: { id: "github", type: "github", name: "GitHub", initials: "GH" },
    server: { id: "server", type: "server", name: "Punchlist", initials: "PL" },
  };

  const spec = {
    running: {
      summary: "People need one place that lists what is waiting on them: issues in Human Review, transitions the server refused, and runs that failed. Today that information is spread across the list, the PR and the runner logs.",
      build: [
        "Add an Inbox route that lists issues where the next transition is by a person, issues whose last transition request was refused, and issues whose last run failed.",
        "Each row names why it is in the inbox and links to the issue page.",
        "The route reads from the existing list query; no new table.",
      ],
      ac: [
        { text: "Opening /inbox shows the three groups with counts.", done: false },
        { text: "An issue in Human Review appears under Waiting on you.", done: false },
        { text: "A refused transition appears with the gate that refused it.", done: false },
        { text: "A failed run appears with the run's outcome line.", done: false },
      ],
    },
    blocked: {
      summary: "The transition API must check roles, locks and gates in one transaction and answer a refused request with the gate that failed and why. Agents loop on that answer, so the reason has to be specific enough to act on.",
      build: [
        "One `POST /issues/:id/transitions` handler that loads the workflow version, checks the actor's role and the target status's lock, runs each gate over recorded evidence and writes the transition in a single transaction.",
        "A refused request returns 409 with the first failing gate, its reason and the evidence it read.",
        "Record each gate result on the transition row.",
      ],
      ac: [
        { text: "A request from an agent into Human Review is refused with `locked`.", done: true },
        { text: "A request with a failing gate is refused with the gate id and a reason.", done: true },
        { text: "A passing request writes the transition and all gate results in one row.", done: true },
        { text: "Two concurrent requests for the same issue produce one transition.", done: false },
      ],
    },
    locked: {
      summary: "Every transition and comment on an issue is an event in an append-only log. The issue page shows the log as a timeline and tells people, agents, runners and GitHub events apart.",
      build: [
        "An `events` table with issue, actor, kind, payload and time, written in the same transaction as the change it records.",
        "A timeline component on the issue page that reads the log and renders transitions, comments, runs and GitHub events.",
      ],
      ac: [
        { text: "A transition writes one event with the actor and the gate results.", done: true },
        { text: "A comment writes one event and shows in the timeline within one refresh.", done: true },
        { text: "The timeline marks person, agent, runner and GitHub actors differently.", done: true },
      ],
    },
    done: {
      summary: "The workflow file is the one place the rules live. Parse `.punchlist/workflow.toml` into typed statuses, transitions, gates, locks and dispatch rules, and refuse files that reference unknown gates or statuses.",
      build: [
        "A `Workflow` type in `punchlist-core` with a parser from TOML.",
        "Validation errors name the table and key that failed.",
      ],
      ac: [
        { text: "The example workflow in the PRD parses to nine statuses and seven gates.", done: true },
        { text: "A transition to an unknown status fails with its key path.", done: true },
        { text: "A gate name that is not defined fails with its key path.", done: true },
      ],
    },
  };

  // Log lines appended while the running issue is shown.
  const liveLog = [
    "[14:02:41] read PL-131 via MCP: 4 acceptance criteria",
    "[14:02:44] git worktree add ../pl-131 -b feature/pl-131-inbox",
    "[14:03:10] cargo check --workspace",
    "[14:03:58] wrote crates/server/src/routes/inbox.rs",
    "[14:04:21] wrote web/src/routes/inbox.tsx",
    "[14:05:02] cargo test -p punchlist-server inbox",
    "[14:05:40] test result: ok. 6 passed; 0 failed",
    "[14:05:52] gh pr create --draft --title \"Inbox for people (PL-131)\"",
    "[14:05:55] opened PR #212 as draft",
    "[14:06:30] running live check 1 of 4: open /inbox at 1280×800",
  ];
  const liveLogMore = [
    "[14:06:48] live check 1 passed: 3 groups, counts 2 / 1 / 1",
    "[14:07:05] running live check 2 of 4: Human Review row under Waiting on you",
    "[14:07:21] live check 2 passed: PL-115 listed",
    "[14:07:30] running live check 3 of 4: refused transition names the gate",
    "[14:07:49] live check 3 passed: PL-121 shows ci_green",
    "[14:08:02] running live check 4 of 4: failed run outcome line",
    "[14:08:19] live check 4 passed: PL-121 run #2 outcome shown",
    "[14:08:25] attaching 4 screenshots as proof",
    "[14:08:40] gh pr ready 212",
  ];

  const issues = [
    {
      id: "PL-131",
      key: "running",
      stateLabel: "Running",
      title: "Inbox for people: issues waiting on a person, refused transitions, failed runs",
      status: "in_progress",
      assignee: "claude",
      labels: ["feature", "web"],
      milestone: "Phase 2: P1 requirements",
      created: "Oct 6",
      spec: spec.running,
      pr: { number: 212, state: "draft", checks: "pending", title: "Inbox for people (PL-131)", branch: "feature/pl-131-inbox", namesIssue: true, mergeable: true, checkRuns: [
        { name: "cargo test", state: "passing", took: "2m 10s" },
        { name: "web build", state: "pending", took: "" },
        { name: "e2e", state: "pending", took: "" },
      ], threads: [] },
      gates: { pr_open: { pass: true, reason: "PR #212 opened 2 min ago" }, pr_ready: { pass: false, reason: "PR #212 is still a draft" }, pr_names_issue: { pass: true, reason: "title names PL-131" } },
      runs: [
        { id: "run-2041", attempt: 1, agent: "claude", runner: "sartre", started: "14:02", duration: "6m 12s", tokens: "184k in · 22k out", cost: "$0.91", outcome: "running", live: true, log: liveLog.slice() },
      ],
      proof: [],
      timeline: [
        { at: "Oct 6, 09:12", actor: "gannon", kind: "transition", from: "backlog", to: "todo" },
        { at: "Oct 7, 13:58", actor: "gannon", kind: "transition", from: "todo", to: "start" },
        { at: "Oct 7, 14:02", actor: "sartre", kind: "claim", text: "claimed the issue, worktree ../pl-131, agent Claude Code" },
        { at: "Oct 7, 14:02", actor: "sartre", kind: "transition", from: "start", to: "in_progress" },
        { at: "Oct 7, 14:05", actor: "github", kind: "pr", text: "pull request #212 opened as draft" },
        { at: "Oct 7, 14:06", actor: "claude", kind: "comment", text: "Draft PR #212 is up. Running the four live checks now, then marking it ready." },
      ],
    },
    {
      id: "PL-121",
      key: "blocked",
      stateLabel: "Blocked",
      title: "Transition API that checks roles, locks and gates in one transaction",
      status: "agent_review",
      assignee: "codex",
      labels: ["feature", "server"],
      milestone: "Gate 1: Groundwork ships",
      created: "Oct 3",
      spec: spec.blocked,
      pr: { number: 207, state: "open", checks: "failing", title: "Transition handler with gates (PL-121)", branch: "feature/pl-121-transition-api", namesIssue: true, mergeable: true, checkRuns: [
        { name: "cargo test", state: "passing", took: "3m 02s" },
        { name: "clippy", state: "passing", took: "1m 11s" },
        { name: "e2e", state: "failing", took: "4m 48s", detail: "2 of 31 failed: transitions::concurrent_requests, transitions::refused_reason" },
      ], threads: [
        { id: "t1", by: "coderabbit", state: "open", path: "crates/server/src/transitions.rs:144", text: "The gate loop runs outside the transaction when `evidence` is empty; a concurrent write can pass a stale gate." },
        { id: "t2", by: "priya", state: "resolved", path: "crates/core/src/gate.rs:30", text: "Rename `Verdict::Ok` to `Verdict::Pass` to match the PRD wording." },
      ] },
      gates: {
        ci_green: { pass: false, reason: "check `e2e` failed on 3f2a1c: 2 of 31 tests" },
        mergeable: { pass: true, reason: "no conflicts with main" },
        no_open_threads: { pass: false, reason: "1 unresolved review thread (CodeRabbit, transitions.rs:144)" },
        proof_attached: { pass: true, reason: "3 proofs attached" },
      },
      refused: { at: "Oct 8, 10:41", by: "codex", to: "human_review", gate: "ci_green" },
      runs: [
        { id: "run-1988", attempt: 1, agent: "claude", runner: "sartre", started: "Oct 7, 09:10", duration: "41m 05s", tokens: "612k in · 48k out", cost: "$3.12", outcome: "succeeded", log: [
          "[09:51:02] gh pr ready 207",
          "[09:51:04] request transition in_progress → agent_review",
          "[09:51:04] server: accepted (pr_open ✓ pr_ready ✓ pr_names_issue ✓)",
        ] },
        { id: "run-2012", attempt: 2, agent: "codex", runner: "mini", started: "Oct 8, 10:12", duration: "29m 40s", tokens: "402k in · 31k out", cost: "$2.40", outcome: "failed", log: [
          "[10:38:10] replied to 2 review threads, resolved 1",
          "[10:40:55] cargo test --workspace: ok",
          "[10:41:12] request transition agent_review → human_review",
          "[10:41:12] server: refused ci_green: check `e2e` failed on 3f2a1c: 2 of 31 tests",
          "[10:41:13] exit 1: transition refused",
        ] },
      ],
      proof: [
        { kind: "checks", title: "cargo test --workspace", meta: "118 passed · 0 failed · 3m 02s", rows: [["transitions::refuses_locked_status", "pass"], ["transitions::records_gate_results", "pass"], ["transitions::single_row_per_request", "pass"]] },
        { kind: "image", title: "Refused request returns 409 with gate and reason", meta: "1280×800 · run-1988", tone: "a" },
        { kind: "image", title: "Transition row with gate results", meta: "1280×800 · run-1988", tone: "b" },
      ],
      timeline: [
        { at: "Oct 3, 11:00", actor: "gannon", kind: "transition", from: "backlog", to: "todo" },
        { at: "Oct 7, 09:08", actor: "gannon", kind: "transition", from: "todo", to: "start" },
        { at: "Oct 7, 09:10", actor: "sartre", kind: "claim", text: "claimed the issue, agent Claude Code" },
        { at: "Oct 7, 09:10", actor: "sartre", kind: "transition", from: "start", to: "in_progress" },
        { at: "Oct 7, 09:22", actor: "github", kind: "pr", text: "pull request #207 opened as draft" },
        { at: "Oct 7, 09:51", actor: "github", kind: "pr", text: "pull request #207 marked ready" },
        { at: "Oct 7, 09:51", actor: "claude", kind: "transition", from: "in_progress", to: "agent_review", gates: ["pr_open", "pr_ready", "pr_names_issue"] },
        { at: "Oct 7, 10:30", actor: "priya", kind: "comment", text: "Left one naming nit on gate.rs. Otherwise the transaction shape matches the ADR." },
        { at: "Oct 8, 10:02", actor: "github", kind: "check", text: "check `e2e` failed on 3f2a1c" },
        { at: "Oct 8, 10:12", actor: "mini", kind: "claim", text: "claimed the issue, agent Codex" },
        { at: "Oct 8, 10:41", actor: "server", kind: "refused", to: "human_review", by: "codex", gate: "ci_green", reason: "check `e2e` failed on 3f2a1c: 2 of 31 tests" },
        { at: "Oct 8, 10:42", actor: "codex", kind: "comment", text: "The two e2e failures are the concurrent-request case. I need the advisory lock on the issue row before the gate loop; fixing in the next run.\n\n(agent)" },
      ],
    },
    {
      id: "PL-115",
      key: "locked",
      stateLabel: "Human Review",
      title: "Append-only event log and timeline per issue",
      status: "human_review",
      assignee: "gannon",
      labels: ["feature", "server", "web"],
      milestone: "Gate 1: Groundwork ships",
      created: "Oct 1",
      spec: spec.locked,
      pr: { number: 198, state: "open", checks: "passing", title: "Event log and issue timeline (PL-115)", branch: "feature/pl-115-event-log", namesIssue: true, mergeable: true, checkRuns: [
        { name: "cargo test", state: "passing", took: "2m 55s" },
        { name: "clippy", state: "passing", took: "1m 03s" },
        { name: "web build", state: "passing", took: "48s" },
        { name: "e2e", state: "passing", took: "5m 12s" },
      ], threads: [
        { id: "t1", by: "coderabbit", state: "resolved", path: "crates/server/src/events.rs:61", text: "Consider an index on (issue_id, created_at)." },
        { id: "t2", by: "codex", state: "resolved", path: "web/src/components/Timeline.tsx:12", text: "Actor kind should come from the event payload, not be inferred from the name." },
      ] },
      gates: {},
      lastGates: { ci_green: { pass: true, reason: "4 of 4 checks passed on 9c1e77" }, mergeable: { pass: true, reason: "no conflicts with main" }, no_open_threads: { pass: true, reason: "2 threads, all resolved" }, proof_attached: { pass: true, reason: "4 proofs attached" } },
      runs: [
        { id: "run-1902", attempt: 1, agent: "claude", runner: "sartre", started: "Oct 2, 15:20", duration: "52m 18s", tokens: "705k in · 61k out", cost: "$3.80", outcome: "succeeded", log: ["[16:12:30] request transition in_progress → agent_review", "[16:12:30] server: accepted"] },
        { id: "run-1931", attempt: 2, agent: "codex", runner: "sartre", started: "Oct 3, 08:05", duration: "18m 44s", tokens: "233k in · 14k out", cost: "$1.22", outcome: "succeeded", log: ["[08:22:10] resolved 2 review threads", "[08:23:40] request transition agent_review → human_review", "[08:23:40] server: accepted (ci_green ✓ mergeable ✓ no_open_threads ✓ proof_attached ✓)"] },
      ],
      proof: [
        { kind: "image", title: "Timeline with person, agent, runner and GitHub entries", meta: "1280×800 · run-1902", tone: "a" },
        { kind: "image", title: "Timeline at 390×844", meta: "390×844 · run-1902", tone: "c" },
        { kind: "video", title: "Comment appears in the timeline after one refresh", meta: "0:42 · run-1902" },
        { kind: "checks", title: "scenario: timeline-actors", meta: "4 passed · 0 failed · 1m 09s", rows: [["person entry shows initials avatar", "pass"], ["agent entry shows bot avatar and kind", "pass"], ["runner entry shows machine name", "pass"], ["GitHub entry shows PR icon", "pass"]] },
      ],
      timeline: [
        { at: "Oct 1, 16:40", actor: "gannon", kind: "transition", from: "backlog", to: "todo" },
        { at: "Oct 2, 15:18", actor: "gannon", kind: "transition", from: "todo", to: "start" },
        { at: "Oct 2, 15:20", actor: "sartre", kind: "claim", text: "claimed the issue, agent Claude Code" },
        { at: "Oct 2, 15:20", actor: "sartre", kind: "transition", from: "start", to: "in_progress" },
        { at: "Oct 2, 15:34", actor: "github", kind: "pr", text: "pull request #198 opened as draft" },
        { at: "Oct 2, 16:12", actor: "github", kind: "pr", text: "pull request #198 marked ready" },
        { at: "Oct 2, 16:12", actor: "claude", kind: "transition", from: "in_progress", to: "agent_review", gates: ["pr_open", "pr_ready", "pr_names_issue"] },
        { at: "Oct 3, 08:05", actor: "sartre", kind: "claim", text: "claimed the issue, agent Codex" },
        { at: "Oct 3, 08:23", actor: "codex", kind: "transition", from: "agent_review", to: "human_review", gates: ["ci_green", "mergeable", "no_open_threads", "proof_attached"] },
        { at: "Oct 3, 08:24", actor: "codex", kind: "comment", text: "All four gates pass. Standing down for Human Review.\n\n(agent)" },
        { at: "Oct 3, 09:10", actor: "priya", kind: "comment", text: "Reviewed the migration; the index CodeRabbit asked for is in. Over to Gannon." },
      ],
    },
    {
      id: "PL-109",
      key: "done",
      stateLabel: "Done",
      title: "Parse the workflow file into typed statuses, transitions, gates and locks",
      status: "done",
      assignee: "claude",
      labels: ["feature", "core"],
      milestone: "Gate 1: Groundwork ships",
      created: "Sep 29",
      spec: spec.done,
      pr: { number: 190, state: "merged", checks: "passing", title: "Workflow parser (PL-109)", branch: "feature/pl-109-workflow-parser", namesIssue: true, mergeable: true, mergedAt: "Oct 1, 11:30", checkRuns: [
        { name: "cargo test", state: "passing", took: "1m 40s" },
        { name: "clippy", state: "passing", took: "58s" },
      ], threads: [
        { id: "t1", by: "coderabbit", state: "resolved", path: "crates/core/src/workflow.rs:88", text: "Unknown gate names should fail at parse time, not at first transition." },
      ] },
      gates: {},
      lastGates: { ci_green: { pass: true, reason: "2 of 2 checks passed on 51ab09" }, mergeable: { pass: true, reason: "no conflicts" }, no_open_threads: { pass: true, reason: "1 thread, resolved" }, proof_attached: { pass: true, reason: "2 proofs attached" } },
      runs: [
        { id: "run-1840", attempt: 1, agent: "claude", runner: "sartre", started: "Sep 30, 10:02", duration: "35m 51s", tokens: "388k in · 40k out", cost: "$2.05", outcome: "succeeded", log: ["[10:36:02] cargo test -p punchlist-core: 23 passed", "[10:37:40] request transition in_progress → agent_review", "[10:37:40] server: accepted"] },
        { id: "run-1861", attempt: 2, agent: "codex", runner: "mini", started: "Sep 30, 14:10", duration: "12m 03s", tokens: "160k in · 9k out", cost: "$0.78", outcome: "succeeded", log: ["[14:20:11] resolved 1 review thread", "[14:22:05] request transition agent_review → human_review", "[14:22:05] server: accepted"] },
      ],
      proof: [
        { kind: "checks", title: "cargo test -p punchlist-core", meta: "23 passed · 0 failed · 1m 40s", rows: [["workflow::parses_prd_example", "pass"], ["workflow::unknown_status_fails_with_path", "pass"], ["workflow::unknown_gate_fails_with_path", "pass"]] },
        { kind: "image", title: "Parse error names the key path", meta: "terminal · run-1840", tone: "b" },
      ],
      timeline: [
        { at: "Sep 29, 12:00", actor: "gannon", kind: "transition", from: "backlog", to: "todo" },
        { at: "Sep 30, 10:00", actor: "gannon", kind: "transition", from: "todo", to: "start" },
        { at: "Sep 30, 10:02", actor: "sartre", kind: "claim", text: "claimed the issue, agent Claude Code" },
        { at: "Sep 30, 10:02", actor: "sartre", kind: "transition", from: "start", to: "in_progress" },
        { at: "Sep 30, 10:15", actor: "github", kind: "pr", text: "pull request #190 opened as draft" },
        { at: "Sep 30, 10:37", actor: "github", kind: "pr", text: "pull request #190 marked ready" },
        { at: "Sep 30, 10:37", actor: "claude", kind: "transition", from: "in_progress", to: "agent_review", gates: ["pr_open", "pr_ready", "pr_names_issue"] },
        { at: "Sep 30, 14:10", actor: "mini", kind: "claim", text: "claimed the issue, agent Codex" },
        { at: "Sep 30, 14:22", actor: "codex", kind: "transition", from: "agent_review", to: "human_review", gates: ["ci_green", "mergeable", "no_open_threads", "proof_attached"] },
        { at: "Oct 1, 11:20", actor: "gannon", kind: "comment", text: "Parser shape is right. Merging." },
        { at: "Oct 1, 11:21", actor: "gannon", kind: "transition", from: "human_review", to: "merging" },
        { at: "Oct 1, 11:30", actor: "github", kind: "pr", text: "pull request #190 merged" },
        { at: "Oct 1, 11:30", actor: "github", kind: "transition", from: "merging", to: "done", event: "pr_merged" },
      ],
    },
  ];

  return { statuses, gates, actors, issues, liveLogMore };
})();
