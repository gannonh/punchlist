// Mock data for the inbox prototype (PL-76). Nothing here is wired to production code.
// Statuses follow `.punchlist/workflow.toml`; the people and agents are made up.

window.PUNCHLIST_INBOX_MOCK = (function () {
  const statuses = {
    backlog: { label: "Backlog" },
    todo: { label: "Todo" },
    start: { label: "Start" },
    in_progress: { label: "In Progress" },
    agent_review: { label: "Agent Review" },
    human_review: { label: "Human Review" },
    merging: { label: "Merging" },
    done: { label: "Done" },
    canceled: { label: "Canceled" },
  };

  // The candidate rule applied to `.punchlist/workflow.toml`: every requestable transition out is `by = ["person"]`.
  const rule = [
    { status: "backlog", out: "to Todo, by person; to Canceled, by person", waits: true },
    { status: "todo", out: "to Start, by person; to Canceled, by person", waits: true },
    { status: "start", out: "to In Progress, by runner", waits: false },
    { status: "in_progress", out: "to Agent Review, by agent", waits: false },
    { status: "agent_review", out: "to Human Review, by agent", waits: false },
    { status: "human_review", out: "to Merging, by person (Done on pr_merged is not a request)", waits: true },
    { status: "merging", out: "to Todo, by person (Done on pr_merged is not a request)", waits: true },
    { status: "done", out: "none: closed", waits: false },
    { status: "canceled", out: "none: closed", waits: false },
  ];

  // kind: waiting | run | refusal. `move` is the one person's move a Move button makes on the mock.
  // Whose item: the assignee when a person, the assignee's owner when an agent, everyone when unassigned.
  const items = [
    { id: "PL-12", kind: "waiting", title: "Record check runs per head commit", status: "human_review", ago: "12 min ago", unread: true,
      why: "Agent Review passed its gates; Human Review waits on a person.", whose: "assigned to Codex, owned by you", move: "merging" },
    { id: "PL-18", kind: "waiting", title: "Import Linear comments", status: "merging", ago: "40 min ago", unread: false,
      why: "In Merging after a gate refused the merge; a person moves it back to Todo.", whose: "assigned to you", move: "todo" },
    { id: "PL-22", kind: "waiting", title: "Filter the board by milestone", status: "todo", ago: "2 h ago", unread: false,
      why: "Approved; waits for Start.", whose: "unassigned: shows to every person", move: "start" },
    { id: "PL-31", kind: "run", title: "Lock Human Review to people", status: "agent_review", ago: "8 min ago", unread: true,
      run: "Codex run failed", reason: "lease expired", whose: "assigned to Codex, owned by you", move: "todo" },
    { id: "PL-40", kind: "run", title: "Resume a Claude Code run after a restart", status: "in_progress", ago: "25 min ago", unread: true,
      run: "Third run ended", reason: "3 runs in In Progress ended without moving it", whose: "assigned to Claude Code, owned by you", move: "todo" },
    { id: "PL-27", kind: "refusal", title: "Show proof on the issue page", status: "agent_review", ago: "5 min ago", unread: true,
      agent: "Codex (codex)", request: "Agent Review → Human Review", gate: "ci_green", reason: "1 of 6 checks failed on 3f2a1c4: test",
      whose: "assigned to Codex, owned by you", move: "todo" },
    { id: "PL-5", kind: "waiting", title: "Inbox across workspaces", status: "backlog", ago: "3 d ago", unread: false, backlog: true,
      why: "A parked idea; waits on a person to approve it.", whose: "unassigned: shows to every person", move: "todo" },
  ];

  return { statuses, rule, items };
})();
