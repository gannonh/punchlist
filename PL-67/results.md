# PL-67 live checks on branch (c96be9e), run pl67-branch-1792798627181

- PASS 1. branch: the thread and the route: [data-thread] 1, route 200
- PASS 2. route shape: {"pull_request":7,"path":"src/lib.rs","line":41,"comments":[{"author":"octocat","body":"Should this return an error?","cut":false}],"comments_total":1,"url":"https://github.com/gannonh/punchlist-sandbox/pull/7#discussion_r1700001"}
- PASS 3. page shows the thread: text "open#7src/lib.rs:41octocat Should this return an error?View on GitHub", href https://github.com/gannonh/punchlist-sandbox/pull/7#discussion_r1700001, target _blank, below #checks true
- PASS 4. cut and total: note true, 10 comments, first "octocat comment 3", last ends "yyyyyy … cut, longer on GitHub", sql "10 | 12 | true"
- PASS 5. plain text and unsafe URL: literal true, img/b 0, window.__x undefined, url NULL t, links 0
- PASS 6. no text recorded: "Text not read yet" after 522 ms, links 0
- PASS 7. live update: 2 threads after 415 ms, 1 after 413 ms; pull_request events: 6 | pull_request, 7 | pull_request, 10 | pull_request, 11 | pull_request
- PASS 10. reach and phone width: agent 200; other workspace review-threads 403, pull-requests 403; scrollWidth 390 <= clientWidth 390
- PASS 8. open pull requests only: route [], "No open review threads." after 519 ms, unresolved rows kept 2
- PASS 9a. Read from GitHub without credentials: button back and the notice shown after 58 ms
- SKIPPED 9b. Read from GitHub with credentials: this run's GitHub token reaches only gannonh/punchlist, so no sandbox pull request or review comment could be made. Tried `gh api repos/gannonh/punchlist-sandbox --jq .full_name`, which answered `Not Found (HTTP 404)`. The GraphQL read that the button triggers, `url` included, is covered by `refreshing_an_issues_pull_requests_reads_its_review_threads_from_github` in crates/punchlist-server/tests/workflow.rs against a fake GitHub.

Check 1 on main (cc01cb8), run pl67-main-1792798581510:
- PASS 1. main: count only, no route: [data-thread] 0, #threads "Review threads · 1 open", route 404
