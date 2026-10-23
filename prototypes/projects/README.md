# Projects prototype

A throwaway prototype of a projects and milestones screen, against mock data only (PL-77). Nothing here is wired to production code. The orchestrator picks one layout; PL-83, PL-84 and PL-85 then wire it.

Open `index.html` in a browser. No build step and no server. `?variant=sections|panes|cards` deep-links a layout, `?milestone=<id>` opens a milestone and `?screen=list` shows the list mock.

| Layout | Key | Shape | Opening a milestone | Forms |
| --- | --- | --- | --- | --- |
| Sections | `1` | One page, each project a section of milestone rows | Its issues open in place under the row | A dialog |
| Two panes | `2` | Projects on the left, the selected project's milestones on the right | The right pane drills into the milestone, with a back button | Inline, in place of what they edit |
| Cards | `3` | Milestone cards in a grid under each project, with a New milestone card | A side sheet over the cards | The same side sheet |

Every layout shows each milestone's name, its `PASS when` text (or `No PASS when yet`), and `n of m done` with a bar split by status, Done first. What counts: an issue in Done is done, and a canceled issue is left out of the total, so `Gate 1: Dogfood` reads `7 of 10 done` with its canceled issue listed under "Left out of the total". An open milestone lists its issues with id, title and status, each linking to `../issue-page/?issue=<id>`.

Every layout creates a project, creates a milestone in a project, renames a project and edits a milestone's name and `PASS when`, on the mock data. A name follows the server's rule (`crates/punchlist-server/src/projects.rs`): trimmed, 1 to 100 characters, unique in the workspace for a project and in the project for a milestone, with the server's own messages (`a milestone needs a name`, `a milestone name is at most 100 characters`). A refused name saves nothing.

The app header mock has a **Projects** item next to **Issues**, the same on the list and the board; `g` then `m` opens the screen and `g` then `i` the list mock. A panel at the top of each layout shows that entry, and how the issue sheet's project and milestone fields link to this screen: the link opens the project or the milestone in the current layout.

Keys: `1` `2` `3` switch layout, `Tab` reaches every milestone, issue link, field and button, `Enter` opens a milestone, `Esc` closes a form or an open milestone.

Styles load `../issue-list/styles.css` and use its theme variables only. Mock data lives in `data.js`.
