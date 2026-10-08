# Issue page prototype

Three throwaway layouts of Punchlist's issue page, against mock data only (KAT-3726). Nothing here is wired to production code, and only the layout Gannon picks becomes the spec. The style, tokens, density and status marks come from the issue-list prototype in `../issue-list/`, whose stylesheet this page loads; the three variants differ in layout, not style.

Open `index.html` in a browser. No build step and no server.

| Layout | Key | Shape |
| --- | --- | --- |
| Sidebar | `1` | Spec, pull request, proof and timeline in the main column; status, transitions, gates and runs in a sticky right rail. On phones the rail stacks above the main column. |
| Tabs | `2` | Header card with status and transitions, then Overview, Runs, Gates, Pull request and Timeline tabs with counts. |
| Pipeline | `3` | The workflow as a stepper across the top with gates drawn between steps; work panels (transitions, runs, gates, proof, PR) on the left and spec plus timeline on the right. |

Four mock issues switch the page between states: **Running** (a run in progress with a live log), **Blocked** (a transition refused by a failing gate), **Human Review** (agents and runners locked out) and **Done**. A "View as" select switches the viewer between a person and an agent, which changes which transitions are allowed and shows the agent-locked notice in Human Review.

Every layout shows the spec and acceptance criteria; the current status and each transition the viewer may make, with a blocked transition naming the failing gate and its reason; the gate checklist with an evidence link per gate; runs with agent, runner, duration, tokens, cost, a log excerpt and the outcome; the linked pull request with checks and review threads; proof (screenshots, a video thumbnail, check results); and a timeline whose entries mark person, agent, runner, GitHub and server actors differently.

Shortcuts are listed in the strip at the bottom: `1` `2` `3` switch layout, `j` and `k` switch the mock issue, `v` toggles the viewer, `g` and `t` jump to gates and timeline, and the Tabs layout adds `[` and `]`. URL parameters `?variant=`, `?state=` and `?as=` deep-link a view.

Mock data lives in `data.js` and follows the example `workflow.toml` in `docs/product/prd.md`.
