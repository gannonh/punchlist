# 1. Define the workflow in TOML and the prompts in Markdown

Date: 2026-10-07. Status: accepted.

## Context

Punchlist's workflow names the statuses, the transitions between them, who may make each transition, the gates that must pass first, the statuses locked to people, and which statuses dispatch an agent. The server parses this definition, validates it, and checks every transition against it.

The Symphony spec puts the workflow in `WORKFLOW.md`: YAML front matter for configuration and a prompt template in the body. That shape is a legacy of Symphony's first version, where every run loaded one system prompt that described the whole workflow. Kata Symphony later moved to separate prompts per stage in `.symphony/prompts/`, which worked better, but the configuration stayed in Markdown front matter.

A state machine that a server enforces is configuration, not prose. A prompt is prose that an agent reads. One file serving both purposes makes the configuration harder to validate and the prompts harder to edit.

Formats considered for the configuration:

- **YAML.** Familiar from CI configs and terse for lists of records. Indentation carries meaning, and scalar typing varies by parser: YAML 1.1 parsers such as PyYAML read a bare `on` as the boolean true, while YAML 1.2 parsers read it as a string. Punchlist's transitions use an `on` key.
- **JSON.** Simplest to parse and validate, but it has no comments, and people will annotate and fork workflow files.
- **TOML.** Comments, explicit types and no meaningful indentation. A list of transitions is written as repeated `[[transition]]` tables, which is wordier than YAML. Deep nesting is awkward, but the workflow is shallow. Rust and TypeScript both have mature parsers, and TOML language servers validate against a JSON Schema.

## Decision

- The workflow lives in `.punchlist/workflow.toml`.
- Prompts live in `.punchlist/prompts/`: `system.md` holds the preamble shared by every run, and each status that dispatches an agent names its own prompt file in `[dispatch.status.<status>]`.
- The server parses the workflow where it enters, at load time, into typed data. A file that fails validation is refused with the line and the reason, and the previous version stays active.
- The server publishes a JSON Schema for `workflow.toml`.
- The workflow version stored on each transition is a content hash over `workflow.toml` and every prompt file it names.

## Consequences

- A Symphony `WORKFLOW.md` does not load directly. A converter (PRD requirement R22, P2) turns its front matter into `workflow.toml` and its body and per-state prompts into prompt files.
- Prompts can change without touching the state machine, and the diff shows which one changed.
- A transition's recorded workflow version identifies the exact rules and prompts in force when it happened.
