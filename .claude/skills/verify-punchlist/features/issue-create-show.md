# Create and show an issue

A person creates an issue with a title and a Markdown body; it gets the next id in the workspace (`PL-1`, `PL-2`, ...) and starts in the workflow's first status, Backlog. Creating it writes a `created` event. `pl issue show` prints the id, title, status, body, pull requests, proof, timeline and runs.

## Sub-features

- `create-id` prints the new id on stdout and nothing else.
- `create-backlog` stores the issue in status `backlog`.
- `create-body` keeps the body verbatim.
- `create-empty-title` refuses a blank title.
- `create-event` writes one `event` row of kind `created`, by the creator; the timeline's first line is `<time>  <actor> (person)  created`.
- `show` prints id and title, `Status: <name>`, any `Parent:`, `Project:` and `Milestone:` lines, the body, `Pull requests`, `Proof`, a `Gates for …` checklist when the next transition is gated, `Timeline` and `Runs`.

## How to get to it (user POV)

- `pl issue create --title <title> [--body <markdown>]`
- `pl issue show <id>`
- `POST /api/issues` with `{"title": ..., "body": ...}`; `GET /api/issues/{id}`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`, no issues yet.

- **Create.** Run `$C pl "$RUN" issue create --title "First" --body "Body"`. Exit `0`, stdout exactly `PL-1`.
- **Show.** Run `$C pl "$RUN" issue show PL-1`. Exit `0`, stdout:

  ```text
  PL-1  First
  Status: Backlog

  Body

  Pull requests
    None linked.

  Proof
    None attached.

  Timeline
    <time>  Verify Person (person)  created

  Runs
    No runs yet.
  ```

  `<time>` is UTC, such as `2026-10-18 17:29:20 UTC`; compare the other lines literally.

- **Stored row.** Run `$C sql "$RUN" "SELECT id, status, title, body FROM issue"`. One row: `PL-1 | backlog | First | Body`. `$C sql "$RUN" "SELECT seq, issue_id, kind FROM event"` has one row, `1 | PL-1 | created`.
- **Blank title.** Run `$C pl "$RUN" issue create --title "  "`. Exit `1`, stderr `error: an issue needs a title (422 Unprocessable Entity)`, and the `issue` table still has one row.

## Gotchas

- Creating an issue writes a `created` event, which takes an event `seq`, so a new issue's timeline is never empty.
- Ids are numbered per workspace and never reused, including after a failed create.
- The title is trimmed before it is stored; the body is stored as given.
- An empty `--body` prints no body section in `show`.
