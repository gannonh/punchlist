# 10. One repository per workspace in v1, with the workflow file in that repository

Date: 2026-10-07. Status: accepted.

## Context

The PRD left two data model questions open:

- Does the workflow file live in the code repository or in the workspace, for teams with several repositories?
- Does a workspace have one repository in v1, or several?

The answers depend on each other. If a workspace has one repository, the workflow file has an obvious home. If it has several, each issue needs a repository, and the workflow needs a home that no single repository owns, or one workflow per repository.

Product principle 2 says the workflow is code, reviewed like code. A file in the repository is changed by a pull request, reviewed and versioned with the code it governs. A workflow kept in the workspace would need its own editor, history and review, which v1 does not have.

Principle 1 says one issue, one branch, one pull request. With one repository per workspace, the runner knows where to create each worktree, and the GitHub App maps each pull request to exactly one workspace. With several, each issue must name its repository, the runner must hold clones of each, and the import must assign repositories.

No Gate 1 requirement needs several repositories. The dogfood target is Groundwork's and Punchlist's own issues, which live in two repositories and can run as two workspaces. The cost is that a person working on both switches between two workspaces. The inbox (R16) is P1 and can span workspaces then.

## Decision

- In v1 a workspace has exactly one GitHub repository.
- The repository is still its own table, referenced by the workspace, so a later phase can allow several without moving data.
- The workflow lives in that repository at `.punchlist/workflow.toml`, with its prompts in `.punchlist/prompts/` (ADR 0001).
- The active workflow is the one on the repository's default branch. When GitHub reports a push to the default branch that changes `.punchlist/`, the server loads and validates the new version. An invalid version is refused with the line and the reason, and the previous version stays active (ADR 0001).
- A change to the workflow on a branch takes effect when it merges, not before.
- A new workspace starts with the built-in default workflow, the template from the PRD, until its repository has a valid `.punchlist/workflow.toml` on the default branch.

## Consequences

- Workflow changes are reviewed in pull requests, and a transition's recorded workflow version is a content hash of files in the repository's history.
- A pull request cannot loosen the gates that judge it, because its workflow changes do not apply until it merges.
- Groundwork and Punchlist dogfood as two workspaces. Moving between them is a workspace switch in the web app.
- The Linear import (R13) needs a filter, such as a Linear project, so that a Linear team spanning several repositories can feed each workspace its own issues.
- Teams with several repositories wait for a later phase. Lifting the limit needs a new ADR that says where the workflow lives when no single repository owns it.
