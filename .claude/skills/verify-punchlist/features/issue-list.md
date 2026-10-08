# List issues

`pl issue list` prints every issue in the workspace, oldest first, one per line as `<id>  <Status>  <title>` with aligned columns, or `No issues.` when there are none.

## Sub-features

- `list-empty` prints `No issues.`.
- `list-rows` prints one aligned row per issue, ordered by number.
- `list-seq` (API only) returns `last_event_seq`, the workspace's latest event number, read in the same snapshot as the list.

## How to get to it (user POV)

- `pl issue list`
- `GET /api/issues`

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`.

- **Empty.** Run `$C pl "$RUN" issue list`. Exit `0`, stdout `No issues.`.
- **Rows.** Create `First` and `Second`, move `PL-1` to `todo` and `start`, then run `$C pl "$RUN" issue list`. Stdout:

  ```text
  PL-1  Start    First
  PL-2  Backlog  Second
  ```

- **Sequence.** `source /tmp/punchlist-verify/$RUN/run.env` and `curl -s -H "Authorization: Bearer $(sed -n 's/^token = "\(.*\)"$/\1/p' "$CONFIG")" "$SERVER_URL/api/issues"`. JSON has `"last_event_seq":2`.

## Gotchas

- Column widths follow the longest id and status name, so the spacing changes with the data.
