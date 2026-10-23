// Mock data for the projects prototype. Nothing here is wired to production code.
// The shape follows `GET /api/projects` (each project, oldest first, with its milestones and their
// `pass_when`) plus each milestone's issues, which the screen would read from `GET /api/issues`.
// Statuses follow the example `workflow.toml` in docs/product/prd.md.

window.PUNCHLIST_PROJECTS_MOCK = (function () {
  const statuses = [
    { id: "backlog", label: "Backlog" },
    { id: "todo", label: "Todo" },
    { id: "start", label: "Start" },
    { id: "in_progress", label: "In Progress" },
    { id: "agent_review", label: "Agent Review" },
    { id: "human_review", label: "Human Review" },
    { id: "merging", label: "Merging" },
    { id: "done", label: "Done" },
    { id: "canceled", label: "Canceled" },
  ];

  const issue = (n, title, status) => ({ id: `PL-${n}`, title, status });

  const projects = [
    {
      id: "p1",
      name: "Punchlist",
      milestones: [
        {
          id: "m1",
          name: "Gate 0: Spec, stack and prototype",
          passWhen: "The PRD, the stack ADRs and the issue list prototype are merged, and pnpm check passes on main.",
          issues: [
            issue(1, "Write the product spec", "done"),
            issue(2, "Pick the license", "done"),
            issue(3, "Pick the backend stack", "done"),
            issue(4, "Pick the frontend stack", "done"),
            issue(5, "Copy the runner from Symphony", "done"),
            issue(6, "Pick the Rust libraries", "done"),
            issue(7, "Pick how the API types reach the web app", "done"),
            issue(8, "Lay out the Cargo workspace", "done"),
            issue(9, "Write the data model ADR", "done"),
            issue(10, "Prototype the issue list and board", "done"),
            issue(11, "Prototype the issue page", "done"),
            issue(12, "Run pnpm check in CI", "done"),
          ],
        },
        {
          id: "m2",
          name: "Gate 1: Dogfood",
          passWhen:
            "Punchlist runs from its own instance: every PL issue moves from Start to Done through a run on sartre.\nA person reviews and merges from the app, not from Linear.\nOne week passes with no issue tracked outside Punchlist.",
          issues: [
            issue(41, "Import the Linear project", "done"),
            issue(48, "Sign in to the app with a person's token", "done"),
            issue(52, "Dispatch Codex in Agent Review", "done"),
            issue(57, "Prototype a pull requests view", "done"),
            issue(63, "Record check runs per head commit", "done"),
            issue(70, "Stream every event to the open app", "done"),
            issue(74, "Run a Codex attempt with its own CODEX_HOME", "done"),
            issue(96, "Moving an issue to Merging dispatches an agent that merges it", "merging"),
            issue(100, "A run acts on GitHub as the workspace's GitHub App", "human_review"),
            issue(103, "Resume a Claude Code attempt after a runner restart", "in_progress"),
            issue(66, "Mirror Punchlist issues back to Linear", "canceled"),
          ],
        },
        {
          id: "m3",
          name: "Phase 2: P1 requirements",
          passWhen:
            "Every P1 requirement in the PRD has a merged slice with its live checks.\nProjects, milestones and the Linear import work from the app without pl.",
          issues: [
            issue(83, "List projects and milestones with their progress", "done"),
            issue(85, "Filter the list and the board by project and milestone", "done"),
            issue(84, "Create a project or a milestone in the app", "human_review"),
            issue(77, "Prototype a projects and milestones screen", "agent_review"),
            issue(104, "Edit a milestone's PASS when in the app", "in_progress"),
            issue(105, "Open a milestone's issues from the projects screen", "in_progress"),
            issue(106, "Archive a finished project", "todo"),
            issue(107, "Show a milestone's target date", "todo"),
            issue(108, "Move every issue of a milestone at once", "todo"),
            issue(109, "Notify a person when a gate fails", "backlog"),
            issue(110, "Search issue bodies and comments", "backlog"),
            issue(111, "Export a milestone as Markdown", "backlog"),
            issue(112, "Show cost per milestone", "backlog"),
            issue(113, "Saved filters on the list", "backlog"),
          ],
        },
      ],
    },
    {
      id: "p2",
      name: "Groundwork",
      milestones: [
        {
          id: "m4",
          name: "Foundation",
          passWhen: "",
          issues: [
            issue(120, "Share the theme variables with Punchlist", "done"),
            issue(121, "Publish the component library", "in_progress"),
            issue(122, "Document the tokens", "todo"),
          ],
        },
      ],
    },
  ];

  return { statuses, projects };
})();
