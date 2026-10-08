# Issue page prototype

A throwaway prototype of Punchlist's issue page, against mock data only (KAT-3726). Nothing here is wired to production code. Gannon approved the **Sidebar** layout on 2026-10-08, so it is the spec; two other layouts (Tabs and Pipeline) were built and dropped. The style, tokens, density and status marks come from the issue-list prototype in `../issue-list/`, whose stylesheet this page loads.

Open `index.html` in a browser. No build step and no server.

**Layout.** Spec, pull request, proof and activity in the main column; status, transitions, gates and runs in a sticky right rail. On phones the rail stacks above the main column, under the issue header.

**States.** Four mock issues switch the page between states: **Running** (a run in progress with a live log), **Blocked** (a transition refused by a failing gate), **Human Review** (agents and runners locked out) and **Done**. A "View as" select switches the viewer between a person and an agent.

**Rules the page demonstrates.**

- Each transition the viewer may make is listed with its verdict. A blocked transition names the first failing gate and its reason; the gate checklist links each gate to its evidence.
- `by` on a transition names the actor that normally drives it. A person may make any move (gates still apply) and sees a note when it is normally an agent's move. An agent or runner may make only its own moves.
- A status lock (Human Review) stops agents and runners acting inside the status: transitions and the comment composer are disabled, and no run dispatches. It does not stop the agent's configured move into the status.
- Runs show agent, model, runner, duration, tokens, cost, outcome and a log excerpt; the running issue's log keeps streaming.
- Activity is the append-only log: transitions with their gate results, claims, GitHub events, refusals and comments, with person, agent, runner, GitHub and server actors marked differently. It has an All / Comments / Transitions filter and a comment composer at the bottom.

**Shortcuts** are listed in the strip at the bottom: `j` and `k` switch the mock issue, `v` toggles the viewer, `g` and `t` jump to gates and activity, `c` focuses the composer. URL parameters `?state=` and `?as=` deep-link a view.

Mock data lives in `data.js` and follows the example `workflow.toml` in `docs/product/prd.md`.
