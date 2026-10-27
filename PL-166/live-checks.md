# PL-166 live checks, branch feature/pl-166 (de79e3f) vs main (ba9686e)

Instances: verify-punchlist `up` under `with-env` (sandbox App), built-in workflow (runs_per_entry 3), runner `r1 --model haiku`; runs resolved `claude-haiku-5-5`.
Agent bodies: request `agent_review` (refused by `pr_open`, no open PR on the issue's branch), sleep 30, request again, end; PL-3: request, then `wait_for_person("Live check: a person must read this.")`, end.
Deviation: the spec's `ci_green` refusal on a Human Review request needs an issue in Agent Review with a pending check; the `pr_open` refusal on In Progress → Agent Review is the same kind of agent gate refusal stored and listed by the same code. PL-1 was skipped: sandbox PR #258 (another run's) is open on `feature/pl-1`, so its request passed.
Poller: every 250 ms, GET /api/inbox as the person and as a second person, the issues' run summaries, and the header's `[data-inbox-count]` in a signed-in headless page (verify-artifacts/lc-branch/poll.jsonl, 3080 samples).

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | main vs branch during the run | PASS | After the first refusal (19:45:11), run `running` on both: main `{"items":[{"kind":"refusal","id":"PL-2","gate":"pr_open","reason":"no open pull request is linked to PL-2"}],"unread":1}`; branch `{"items":[],"unread":0}` |
| 2 | header during the run | PASS | Header `0` from 19:45:03 to 19:51:27 through refusals at 19:45:11, 19:50:29, 19:50:45 (round 1) and 19:52:25, 19:53:14, 19:53:30 (round 2); PL-3's refusal at 19:53:43 left it at `2` (PL-2's two items) |
| 3 | run ends on a refusal | PASS | Third run finished 19:51:27.341; API listed `run:PL-2, refusal:PL-2` at 19:51:27.410 (69 ms), header `2` at 19:51:27.978 (637 ms). Item: gate `pr_open`, reason `no open pull request is linked to PL-2`, agent `Claude Code (claude-code)`, move `In Progress → Agent Review` |
| 4 | Run again clears it | PASS | Click 19:52:20.474, one `POST /api/issues/PL-2/retry` at 19:52:20.499, run 4 started 19:52:20.505; next screen sample 19:52:20.751 has no items, header `0`. URL stays `/w/PL/inbox`. Round 2's end (19:53:37.345) listed the refusal again at 19:53:37.494 |
| 5 | waiting wins | PASS | PL-3: refusal (seq 45), comment from wait_for_person (46), run ended (47); inbox has one PL-3 item: `{"kind":"run",...,"waiting_reason":"Live check: a person must read this.","limit_reached":false}`, no refusal item |
| 6 | count | PASS | Settled header vs items: 0/0, 2/2, 0/0, 2/2, 3/3, 1/1; the header follows within about 520 ms of the API |
| 7 | owner only | PASS | Second person's inbox never had a `refusal` or `run` item in 3080 samples. It showed `waiting:PL-1` and `waiting:PL-2` for one sample each while those issues sat in Todo (every person sees an unassigned Todo issue), before any run |
| 8 | issue moves | PASS | Refusal listed; sandbox PR #308 `Live check for PL-166 (PL-2)` opened; a standalone agent's `request-transition PL-2 agent_review` printed `Moved PL-2 to Agent Review.` (transition in_progress → agent_review 19:54:07.768); inbox then `["run:PL-3"]`, 1. PR #308 closed, branch deleted |
| 9 | server test | PASS | `cargo test -p punchlist-server --test inbox`: 8 passed, including `a_refusal_is_listed_only_once_the_run_has_ended_on_it` and `an_issue_refused_then_waiting_on_a_person_is_listed_once_as_waiting` (both fail on main's inbox.rs) |
| 10 | phone | PASS | 390 px: document scrollWidth 390 = clientWidth 390; refusal item scrollWidth 364 = clientWidth 364 |
