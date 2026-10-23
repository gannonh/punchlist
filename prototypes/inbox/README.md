# Inbox prototype

A throwaway prototype of the inbox (PRD R16), against mock data only (PL-76). Nothing here is wired to production code. The inbox gathers what needs a person in one workspace: issues waiting on a person, runs that need a person, and transitions an agent was refused. The orchestrator picks one layout; then PL-79, PL-80 and PL-82 wire it.

Open `index.html` in a browser. No build step and no server. `?variant=page|split|panel` deep-links a layout. Switching layouts resets the mock to its load state: 6 items, 4 unread.

| Layout | Key | Shape | Done with it | Reached from the list and the board |
| --- | --- | --- | --- | --- |
| Page | `1` | A screen of its own, with a section per kind | Acting clears it: an item leaves when its condition ends. Opening marks it read. No dismiss | An Inbox item with the unread count next to Issues in the app header; `g` `i` |
| Two-pane | `2` | The items on the left, the selected item's detail on the right; stacks below 768 px | Dismiss: acting clears it, and Dismiss hides it until its condition arises again | An Inbox entry with the unread count at the top of a sidebar; `g` `i` |
| Panel | `3` | A panel over the list or the board | Mark read, keep visible: an item stays, with its kind and the status it moved to; Mark all read clears the count only | An Inbox button with the unread count in the top bar; `i` |

Every item shows its kind, the issue's id, title and status, how long ago it arose, whose item it is, an **Open** action and a **Move** action (a mock person's move). A run item shows what happened and why. A refusal shows the agent that asked, the move, the gate and the reason as plain text.

The three kinds of item:

1. **Waiting on you.** The issue sits in a status that only a person moves it out of.
2. **Run needs you.** The issue's latest run failed, or its third run in a status ended without moving it.
3. **Refused transition.** An agent asked for a move and a gate refused it.

Each layout has a rule panel. An issue waits on a person when every transition out of its status that a caller can request is `by = ["person"]` and no other role. A transition an event makes (`on = "pr_merged"`) does not count, and a closed status never qualifies. In `.punchlist/workflow.toml` that gives Backlog, Todo, Human Review and Merging. The **Include Backlog** switch, off at load, adds `PL-5`. Backlog is left out by default (decided 2026-10-23), because it holds every parked idea and would bury the inbox.

Whose item: an issue shows to its assignee when that is a person, to the owner of its assignee when that is an agent, and to every person when it is unassigned.

Items and actions are in the Tab order. `Enter` on an item opens its issue (a toast in the prototype) and marks it read.

Styles load `../issue-list/styles.css` and use its theme variables only. Mock data lives in `data.js`.
