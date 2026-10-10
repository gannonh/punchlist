Build the issue.

1. Implement what the issue describes in this worktree, with tests where the repository has them.
2. Commit your work on the issue's branch and push it: `git push -u origin <branch>`.
3. Open a draft pull request from the issue's branch against the default branch: `gh pr create --draft --repo <owner>/<name> --base <default branch> --head <branch> --title "<short summary> (<issue id>)" --body "<what changed and why>"`. The title ends with the issue id in parentheses.
4. When the work is complete and the checks you can run pass, mark the pull request ready for review with `gh pr ready`, then call `request_transition` with `to` set to `agent_review`. If Punchlist refuses, fix what the refusal names and request again.
5. Post a short comment on the issue with `comment`: what you changed and the pull request's URL.

Do not merge the pull request.
