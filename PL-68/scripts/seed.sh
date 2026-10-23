# seed.sh: 500 issues, 50 open pull requests on PL-1..PL-50, 5000 closed ones. REPO and RUN set.
set -euo pipefail
export PUNCHLIST_DB_PORT=5468 TMPDIR=~/.cache/pl68-tmp
C=$REPO/.claude/skills/verify-punchlist/bin/control-punchlist
source $TMPDIR/punchlist-verify/$RUN/run.env
node ~/.cache/pl68-live/seed.mjs "$SERVER_URL" "$CONFIG"
$C sql "$RUN" "INSERT INTO pull_request (repository_id, number, github_id, title, branch, head_sha, url, author_login, state, draft, issue_id, github_updated_at)
  SELECT r.id, n, n, 'Seeded open ' || n, 'feature/pl-' || n || '-seed', lpad(n::text, 40, '0'),
         'https://github.com/gannonh/punchlist-sandbox/pull/' || n, 'gannonh', 'open', n % 5 = 0, 'PL-' || n, now()
  FROM repository r, generate_series(1, 50) n"
$C sql "$RUN" "INSERT INTO pull_request (repository_id, number, github_id, title, branch, head_sha, url, author_login, state, draft, issue_id, github_updated_at)
  SELECT r.id, n, n, 'Seeded closed ' || n, 'old/' || n, lpad(n::text, 40, '0'),
         'https://github.com/gannonh/punchlist-sandbox/pull/' || n, 'gannonh', CASE WHEN n % 2 = 0 THEN 'merged' ELSE 'closed' END, false,
         CASE WHEN n % 3 = 0 THEN 'PL-' || (n % 500 + 1) END, now()
  FROM repository r, generate_series(100, 5099) n"
$C sql "$RUN" "SELECT state, count(*) FROM pull_request GROUP BY state ORDER BY state"
$C sql "$RUN" "SELECT count(*) AS issues FROM issue"
