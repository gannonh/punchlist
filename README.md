# Punchlist

An open-source issue tracker where coding agents do the work and the tracker enforces how the work gets done.

A runner on your own machine claims issues that are ready, runs Claude Code, Codex, OpenCode, Pi or Cursor in a worktree per issue, opens a pull request, and moves the issue through your workflow. The workflow lives in `.punchlist/workflow.toml`: the statuses, who may move an issue between them, and the gates that must pass first, such as CI green, review threads resolved and proof attached. The server checks every transition against that file, so an agent cannot skip a gate or act on an issue that is waiting for a person.

## Status

Pre-alpha. There is no code yet, only the spec and the decisions behind it:

- [Product vision and v1 PRD](docs/product/prd.md)
- [ADR 0001: Workflow in TOML, prompts in Markdown](docs/adr/0001-workflow-config-and-prompts.md)
- [ADR 0002: MIT or Apache-2.0](docs/adr/0002-dual-license.md)
- [ADR 0003: Rust backend](docs/adr/0003-rust-backend.md)
- [ADR 0004: Web frontend](docs/adr/0004-web-frontend.md)
- [ADR 0005: Runner copied from Symphony, state on the server](docs/adr/0005-runner-from-symphony.md)
- [ADR 0006: Server state with optimistic updates](docs/adr/0006-server-state-with-optimistic-updates.md)
- [ADR 0007: Rust libraries](docs/adr/0007-rust-libraries.md)
- [ADR 0008: API types through OpenAPI](docs/adr/0008-api-types-through-openapi.md)
- [ADR 0009: Repository layout and commands](docs/adr/0009-repo-layout.md)
- [ADR 0010: One repository per workspace](docs/adr/0010-one-repository-per-workspace.md)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
