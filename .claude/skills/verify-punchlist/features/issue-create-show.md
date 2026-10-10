# Create and show an issue

A person creates an issue with a title and a Markdown body; it gets the next id in the workspace (`PL-1`, `PL-2`, ...) and starts in the workflow's first status, Backlog. `pl issue show` prints the id, title, status, body, pull requests, timeline and runs.

## Sub-features

- `create-id` prints the new id on stdout and nothing else.
- `create-backlog` stores the issue in status `backlog`.
- `create-body` keeps the body verbatim.
- `create-empty-title` refuses a blank title.
- `show` prints id and title, `Status: <name>`, the body, then the `Pull requests`, `Timeline` and `Runs` sections.

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

  Timeline
    No transitions yet.

  Runs
    No runs yet.
  ```

- **Stored row.** Run `$C sql "$RUN" "SELECT id, status, title, body FROM issue"`. One row: `PL-1 | backlog | First | Body`.
- **Blank title.** Run `$C pl "$RUN" issue create --title "  "`. Exit `1`, stderr `error: an issue needs a title (422 Unprocessable Entity)`, and the `issue` table still has one row.

## Gotchas

- Creating an issue writes no `event` row; the timeline holds transitions and comments, so a new issue shows `No transitions yet.`
- Ids are numbered per workspace. A refused create uses no number: after the blank title above, the next issue is `PL-2`.
- The title is trimmed before it is stored; the body is stored as given.
- An empty `--body` prints no body section in `show`.
