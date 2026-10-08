# 8. Generate the TypeScript API client from an OpenAPI document derived from the Rust code

Date: 2026-10-07. Status: accepted.

## Context

ADR 0004 says the web app's API types are generated from the Rust types so the client and server cannot drift. Two ways to do that:

- **Export types directly.** ts-rs or specta derive a TypeScript declaration for each Rust struct and enum. They cover the shapes of requests and responses, not the routes: the client still writes each path, method and status code by hand, and those can drift.
- **Derive an OpenAPI document from the Rust code.** utoipa derives schemas from the Rust types, and utoipa-axum registers each handler's path, method, parameters and responses as the router is built. openapi-typescript turns the document into TypeScript types, and openapi-fetch is a small client that checks each call's path, parameters, body and response against them.

Agents and other clients use the same API (principle 6). A published OpenAPI document describes it for them as well. The `pl` CLI, the MCP server and the runner are Rust and use the Rust types directly.

## Decision

- Request and response types live in `punchlist-api`, a crate with serde and utoipa derives and no I/O. The server and the Rust client both depend on it.
- Handlers are registered through utoipa-axum, so the OpenAPI 3.1 document comes from the same router the server runs.
- `pnpm gen:api` writes the document to `web/src/api/openapi.json` and runs openapi-typescript to write `web/src/api/schema.d.ts`. Both files are committed.
- The web app calls the API only through openapi-fetch typed with `schema.d.ts`.
- CI regenerates both files and fails if they differ from the commit.
- The server serves the document at `GET /api/openapi.json`.

## Consequences

- A change to a Rust type or route that the web app does not handle fails `tsc` before it ships.
- The diff of `openapi.json` in a pull request shows every API change in one place.
- utoipa derives sit beside serde derives on API types, and the two must agree. utoipa reads serde's attributes such as `rename_all` and `tag`, and CI catches the rest when the generated client stops compiling.
- `punchlist-core` stays free of API concerns. A core type that crosses the API is wrapped or mirrored in `punchlist-api`.
