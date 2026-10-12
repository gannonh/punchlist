Get the issue's pull request ready for a person to review. The pull request is open from this branch.

1. Read the pull request's checks with `gh pr checks <number> --repo <owner>/<name>`, and its review threads with `gh api graphql -f query='query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved path line comments(first:20){nodes{databaseId author{login} body}}}}}}}' -F owner=<owner> -F name=<name> -F number=<number>`.
2. Fix CI. For each failing check, read its log with `gh run view <run id> --repo <owner>/<name> --log-failed`, fix the cause in this worktree, commit and push.
3. Answer every unresolved review thread. Where the reviewer is right, fix the code, push, and reply with what changed. Where the finding is a false positive, reply with the reason. Reply with `gh api repos/<owner>/<name>/pulls/<number>/comments/<comment id>/replies -f body=<reply>`, then resolve the thread with `gh api graphql -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}' -F id=<thread id>`.
4. Post a short comment on the issue with `comment`: what you fixed and how you answered each thread.
5. Call `request_transition` with `to` set to `human_review`. If Punchlist refuses, fix what the refusal names and request again. When the refusal says to request again, wait a few seconds and do that.

Do not merge the pull request.
