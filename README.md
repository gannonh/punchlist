# Punchlist

An open-source issue tracker where coding agents do the work and the tracker enforces how the work gets done.

A runner on your own machine claims issues that are ready, runs Claude Code, Codex, OpenCode, Pi or Cursor in a worktree per issue, opens a pull request, and moves the issue through your workflow. The workflow lives in `.punchlist/workflow.toml`: the statuses, who may move an issue between them, and the gates that must pass first, such as CI green, review threads resolved and proof attached. The server checks every transition against that file, so an agent cannot skip a gate or act on an issue that is waiting for a person.

## Status

Pre-alpha. The server, the web app, the `pl` CLI and the runner work end to end, and Punchlist runs its own issues.

## Self-host with Docker Compose

You need Docker with Compose. From a checkout:

```sh
docker compose up -d --build        # Postgres and the server, which runs migrations at start
```

The app is at `http://127.0.0.1:7878`. Create a workspace and a person, and save the printed `pl` config:

```sh
(umask 077 && mkdir -p ~/.config/punchlist &&
  docker compose exec -T server punchlist-server bootstrap --person "Your Name" \
    --repository owner/name --server-url http://127.0.0.1:7878 > ~/.config/punchlist/config.toml)
```

The image carries `pl` for Linux: `mkdir -p ~/.local/bin && docker compose cp server:/usr/local/bin/pl ~/.local/bin/pl`. Runners run `pl runner start` on the machines that have the agents; they are not part of the stack.

Compose reads these variables from the environment it runs in:

| Variable | Default | Purpose |
| --- | --- | --- |
| `PUNCHLIST_HOST` | `127.0.0.1` | The host address the app's port is published on, such as a Tailscale address |
| `PUNCHLIST_PORT` | `7878` | The app's port |
| `PUNCHLIST_DB_PORT` | `5433` | Postgres on `127.0.0.1`, for backups and `psql` |
| `GITHUB_APP_ID` | none | The GitHub App's id; with the key, the server reads `.punchlist/` and pull requests |
| `GITHUB_APP_PRIVATE_KEY` | none | The App's private key, as PEM. A secret |
| `GITHUB_WEBHOOK_SECRET` | none | The App's webhook secret; without it the webhook route answers 503. A secret |
| `PUNCHLIST_PROOF_MAX_BYTES` | `104857600` | The largest proof file accepted |
| `RUST_LOG` | `info,sqlx=warn` | Log filter |

Keep secrets out of files: start the stack from a shell that has them, such as `with-env docker compose up -d`. GitHub must reach `https://<host>/api/github/webhook`; on a tailnet, a Tailscale Funnel on that one path does it. Over https, put the server behind a proxy that sets `X-Forwarded-Proto: https`, so the session cookie is `Secure`.

Postgres keeps its data in the `db` volume and the server keeps proof files in the `proof` volume. `docker compose restart` and `docker compose up -d --build` keep both; `docker compose down -v` deletes both.

### Back up and restore

Back up both volumes:

```sh
docker compose exec -T db pg_dump -U punchlist -Fc punchlist > punchlist.dump
docker compose exec -T server tar -C /var/lib/punchlist -cf - proof > punchlist-proof.tar
```

Restore them, into the same stack or a new one:

```sh
docker compose stop server
docker compose up -d --wait db
docker compose exec -T db pg_restore -U punchlist -d punchlist --clean --if-exists < punchlist.dump
docker compose run --rm -T --no-deps --entrypoint sh server -c 'rm -rf /var/lib/punchlist/proof/* && tar -C /var/lib/punchlist -xf -' < punchlist-proof.tar
docker compose up -d
```

`pnpm e2e:compose` checks all of this: it builds the image, starts the stack on throwaway volumes, creates an issue with proof through `pl`, restarts the stack, backs it up, restores the backup into new volumes, and checks the issue, its proof file and the web app after each step.

## Develop

For development you need Rust (through [rustup](https://rustup.rs), which reads `rust-toolchain.toml`), Node 24, pnpm and Docker.

```sh
pnpm dev                                    # Postgres on 127.0.0.1:5433, then the server on 127.0.0.1:7878
```

In another terminal, create a workspace and a person, and save the printed config. It holds the person's token, so only you can read it:

```sh
umask 077 && mkdir -p ~/.config/punchlist &&
DATABASE_URL=postgres://punchlist:punchlist@127.0.0.1:5433/punchlist \
cargo run -q -p punchlist-server -- bootstrap --person "Your Name" > ~/.config/punchlist/config.toml
```

Then drive it with `pl`:

```sh
cargo run -q -p punchlist-cli -- issue create --title "First" --body "Body"   # PL-1
cargo run -q -p punchlist-cli -- issue move PL-1 todo
cargo run -q -p punchlist-cli -- issue show PL-1
cargo run -q -p punchlist-cli -- issue list
```

`pnpm check` runs formatting, clippy, the tests and the generated-file checks. `docker compose down -v` removes the database.

## Docs

- [Dogfood checklist](docs/dogfood.md)
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
