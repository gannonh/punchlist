// Mock data for the pull requests prototype. Nothing here is wired to production code.
// Statuses follow the example `workflow.toml` in docs/product/prd.md; merge states are
// GitHub's `mergeStateStatus` values, lower-cased as Punchlist records them.

window.PUNCHLIST_PR_MOCK = (function () {
  const repo = { owner: "gannonh", name: "punchlist-sandbox", defaultBranch: "main" };

  const statuses = {
    in_progress: { label: "In Progress", order: 3 },
    agent_review: { label: "Agent Review", order: 4 },
    human_review: { label: "Human Review", order: 5 },
    merging: { label: "Merging", order: 6 },
  };

  // How each merge state reads, and whether it stops a merge on GitHub.
  const mergeStates = {
    clean: { label: "Ready to merge", tone: "ok" },
    has_hooks: { label: "Ready to merge", tone: "ok" },
    unstable: { label: "Checks failing", tone: "bad" },
    blocked: { label: "Blocked by review", tone: "warn" },
    behind: { label: "Behind main", tone: "warn" },
    dirty: { label: "Conflicts", tone: "bad" },
    draft: { label: "Draft", tone: "muted" },
  };

  // Every open or draft pull request in the workspace's repository. `issue: null` names no issue.
  const pullRequests = [
    { number: 241, title: "Show the run log on the issue page", branch: "feature/pl-1", draft: false, issue: { id: "PL-1", title: "Show the run log on the issue page", status: "merging" }, checks: { passed: 6, total: 6 }, threads: 0, merge: "clean", updated: "4 min ago" },
    { number: 238, title: "Refuse a transition that skips a gate", branch: "feature/pl-3", draft: false, issue: { id: "PL-3", title: "Refuse a transition that skips a gate", status: "merging" }, checks: { passed: 6, total: 6 }, threads: 0, merge: "dirty", updated: "1 h ago" },
    { number: 244, title: "Lock Human Review to people", branch: "feature/pl-4", draft: false, issue: { id: "PL-4", title: "Lock Human Review to people", status: "human_review" }, checks: { passed: 5, total: 6 }, threads: 2, merge: "blocked", updated: "22 min ago" },
    { number: 246, title: "Dispatch Codex in Agent Review", branch: "feature/pl-7", draft: false, issue: { id: "PL-7", title: "Dispatch Codex in Agent Review", status: "agent_review" }, checks: { passed: 4, total: 6 }, threads: 1, merge: "unstable", updated: "9 min ago" },
    { number: 250, title: "Record check runs per head commit", branch: "feature/pl-12", draft: false, issue: { id: "PL-12", title: "Record check runs per head commit", status: "agent_review" }, checks: { passed: 6, total: 6 }, threads: 0, merge: "behind", updated: "35 min ago" },
    { number: 247, title: "Import Linear comments", branch: "feature/pl-9", draft: true, issue: { id: "PL-9", title: "Import Linear comments", status: "in_progress" }, checks: { passed: 2, total: 6 }, threads: 0, merge: "draft", updated: "2 min ago" },
    { number: 249, title: "Bump tokio and axum", branch: "chore/bump-deps", draft: false, issue: null, checks: { passed: 6, total: 6 }, threads: 0, merge: "clean", updated: "3 h ago" },
  ];

  for (const pr of pullRequests) pr.url = `https://github.com/${repo.owner}/${repo.name}/pull/${pr.number}`;

  return { repo, statuses, mergeStates, pullRequests };
})();
