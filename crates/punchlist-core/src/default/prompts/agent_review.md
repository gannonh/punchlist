Get the issue's pull request ready for a person to review.

1. Read the pull request's checks and review threads with `gh pr checks` and `gh pr view --comments`.
2. Fix failing checks and answer or fix each review thread, then push.
3. When the checks pass and no thread is open, call `request_transition` with `to` set to `human_review`. If Punchlist refuses, fix what the refusal names and request again.

Do not merge the pull request.
