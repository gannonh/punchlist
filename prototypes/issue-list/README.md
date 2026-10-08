# Issue list prototype

A throwaway prototype of Punchlist's home screen, against mock data only (KAT-3725). Nothing here is wired to production code. Gannon approved both layouts on 2026-10-08, so both are the spec. A third variant, Cards, was built and dropped.

Open `index.html` in a browser. No build step and no server.

| Layout | Key | Shape |
| --- | --- | --- |
| List | `1` | Dense rows grouped by status, one issue per line |
| Board | `2` | One column per status; on narrow screens one column at a time with a status strip |

Both layouts show, per issue: status group, assignee (person or agent, with the agent's kind), run state (queued, running, failed, idle), pull request state with its checks, and a gate summary such as "3 of 4 gates pass". Statuses locked to people (Human Review in the PRD's example workflow) carry a "people only" badge.

Shortcuts are listed in the strip at the bottom of each layout: `j` and `k` move, `Enter` opens, `s` changes status, `/` filters, `1` and `2` switch layouts, and the board adds `h` and `l` for columns. The filter box and assignee select produce the empty state when nothing matches.

Mock data lives in `data.js` and follows the example `workflow.toml` in `docs/product/prd.md`.
