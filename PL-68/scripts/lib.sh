# Live-check helpers for PL-68. Source with REPO, RUN and C set.
FIX=$REPO/crates/punchlist-server/tests/fixtures/github
LIVE=~/.cache/pl68-live
# A pull_request delivery: pr ACTION FIXTURE NUMBER BRANCH SHA TITLE DRAFT
pr() {
  local action=$1 fixture=$2 n=$3 branch=$4 sha=$5 title=$6 draft=$7
  local tick; tick=$(( $(cat "$LIVE/tick" 2>/dev/null || echo 0) + 1 )); echo "$tick" > "$LIVE/tick"
  local at; at=$(date -u -d "2026-10-23T12:00:00Z + $tick seconds" +%FT%TZ)
  jq --arg action "$action" --argjson n "$n" --arg branch "$branch" --arg sha "$sha" --arg title "$title" \
     --argjson draft "$draft" --arg at "$at" '
    (.. | strings) |= sub("gannonh/punchlist(?<e>/|$)"; "gannonh/punchlist-sandbox\(.e)")
    | .repository.name = "punchlist-sandbox"
    | .action = $action | .number = $n
    | .pull_request.number = $n | .pull_request.head.ref = $branch | .pull_request.head.sha = $sha
    | .pull_request.title = $title | .pull_request.draft = $draft | .pull_request.updated_at = $at
    | .pull_request.html_url = "https://github.com/gannonh/punchlist-sandbox/pull/\($n)"' \
    "$FIX/$fixture.json" > "$LIVE/payload-$n-$tick.json"
  $C webhook "$RUN" pull_request "$LIVE/payload-$n-$tick.json" "pl68-$n-$tick-$RANDOM"
}
sha() { printf '%040d' "$1"; }
# A check_run completed delivery for pull request NUMBER on BRANCH at SHA: check NUMBER BRANCH SHA CONCLUSION
check() {
  jq --argjson n "$1" --arg branch "$2" --arg sha "$3" --arg c "$4" '
    (.. | strings) |= sub("gannonh/punchlist(?<e>/|$)"; "gannonh/punchlist-sandbox\(.e)")
    | .repository.name = "punchlist-sandbox" | .check_run.head_sha = $sha | .check_run.conclusion = $c
    | .check_run.pull_requests[0].number = $n | .check_run.pull_requests[0].head.ref = $branch
    | .check_run.pull_requests[0].head.sha = $sha
    | .check_run.pull_requests[0].head.repo.name = "punchlist-sandbox"
    | .check_run.pull_requests[0].base.repo.name = "punchlist-sandbox"' \
    "$FIX/check_run_completed.json" > "$LIVE/check-$1.json"
  $C webhook "$RUN" check_run "$LIVE/check-$1.json" "pl68-check-$1-$RANDOM"
}
# A review thread delivery: thread ACTION NUMBER BRANCH SHA TITLE
thread() {
  local tick; tick=$(( $(cat "$LIVE/tick" 2>/dev/null || echo 0) + 1 )); echo "$tick" > "$LIVE/tick"
  local at; at=$(date -u -d "2026-10-23T12:00:00Z + $tick seconds" +%FT%TZ)
  jq --arg action "$1" --argjson n "$2" --arg branch "$3" --arg sha "$4" --arg title "$5" --arg at "$at" '
    (.. | strings) |= sub("gannonh/punchlist(?<e>/|$)"; "gannonh/punchlist-sandbox\(.e)")
    | .repository.name = "punchlist-sandbox" | .action = $action
    | .pull_request.number = $n | .pull_request.head.ref = $branch | .pull_request.head.sha = $sha
    | .pull_request.title = $title | .pull_request.draft = false | .pull_request.updated_at = $at
    | .pull_request.html_url = "https://github.com/gannonh/punchlist-sandbox/pull/\($n)"' \
    "$FIX/pull_request_review_thread_resolved.json" > "$LIVE/thread-$2.json"
  $C webhook "$RUN" pull_request_review_thread "$LIVE/thread-$2.json" "pl68-thread-$2-$RANDOM"
}
