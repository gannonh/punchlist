# 6. Keep state on the server, with optimistic edits and a live event stream

Date: 2026-10-07. Status: accepted.

## Context

Product principle 6 asks for a web app that responds instantly, and the PRD sets a target: lists and issue pages respond in under 100 ms after first load. ADR 0004 says that speed comes from local data and writes that show before the server confirms them, and it left the approach to local data open. The choice sets the shape of the server API.

Two approaches:

- **A sync engine** keeps a replica of the data in the browser and syncs it with Postgres.
  - Zero (Rocicorp, 1.0 in 2026) runs a `zero-cache` service that replicates Postgres. Writes are custom mutators that run once on the client and again in the app's push endpoint. Rocicorp's server tooling for mutators is TypeScript.
  - Electric syncs the read path only, as shapes over HTTP, and leaves writes to the app's own API. Shape access is enforced in a proxy the app writes.
- **Server state with optimistic updates.** The client fetches data through the API into a cache, applies its own writes to the cache before the server answers, and rolls back on refusal. A stream of server events keeps the cache current.

Punchlist's writes are not plain edits. A transition checks roles, locks and gates in one Postgres transaction in the Rust server (ADR 0003), and the gates read evidence the client does not hold. With Zero, the mutator that runs in the push endpoint is TypeScript, so either the transition rules exist twice or the mutator calls the Rust server and the sync engine adds nothing to writes. Either sync engine adds a service and Postgres logical replication to the self-host stack (R20).

The data is small. A workspace has 1 to 10 people and a few thousand issues, so one workspace's issue list fits in the browser's memory in a single response. The event log (R6) is append-only, so it is already a change feed.

## Decision

- No sync engine in v1. The server is the only store, and the client holds a cache.
- The client uses TanStack Query for the cache and TanStack Router for routes, as Groundwork does.
- On first load the client fetches the workspace's full issue list in one request. Lists, filters and the board read from the cache.
- Each event in the log has a sequence number that increases per workspace. The server streams new events over Server-Sent Events at `GET /api/workspaces/{id}/events?after={seq}`. The client applies each event to its cache and reconnects from the last sequence it saw.
- The issue list response carries the sequence number of the last event it includes, read in the same database snapshot as the list. The client opens the stream with that number as `after`, so no event is skipped or applied twice between the list and the stream.
- Edits to an issue's fields (title, body, labels, assignee, project, milestone, parent) are optimistic: the cache changes at once and rolls back if the server refuses.
- Transitions are not optimistic. The issue shows the requested status as pending until the server answers, then settles or shows the failing gate and its reason.
- The API is resource-shaped JSON over HTTP. Every write returns the changed resource and the sequence number of the event it recorded, so the client can drop that event when the stream delivers it.

## Consequences

- One implementation of the transition rules, in Rust, and no extra service to self-host.
- Navigation and filtering after first load read from memory and meet the 100 ms target. First load costs one request whose size grows with the workspace.
- A refused transition never shows a card moving to a column and back.
- An offline client cannot write. Offline edits are not a v1 requirement.
- The event stream doubles as the change feed for the issue timeline and the run logs on the issue page.
- If workspaces outgrow a single list request, the sequence-numbered log is the starting point for a sync engine or for paged sync. That would be a new ADR.
