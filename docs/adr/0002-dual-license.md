# 2. License under MIT or Apache-2.0

Date: 2026-10-07. Status: accepted.

## Context

Gannon wants a permissive license, MIT or Apache-2.0. The PRD's draft said MIT, copying Groundwork's ADR 0005.

The two licenses differ in ways that matter to some users. Apache-2.0 grants an explicit patent license and spells out the terms for contributions. MIT is shorter and is compatible with GPLv2, which Apache-2.0 is not. Most Rust projects, including the Rust compiler, offer both and let each user choose. Punchlist's backend is Rust (ADR 0003). Its likely dependencies use the same licenses: Kata Symphony and OpenAI Symphony are Apache-2.0, as are GPUI and GPUI Kit.

## Decision

- Punchlist is dual-licensed: `MIT OR Apache-2.0`. `LICENSE-MIT` and `LICENSE-APACHE` hold the texts, and every crate's `Cargo.toml` sets `license = "MIT OR Apache-2.0"`.
- No contributor license agreement. The README states the Rust convention: a contribution is dual-licensed under the same terms unless its author says otherwise.

## Consequences

- Each user picks the license that suits them: the patent grant from Apache-2.0, or GPLv2 compatibility from MIT.
- As with Groundwork, anyone may run Punchlist as a hosted product and keep their changes private. The hosted cloud competes on running the product well.
- Gannon holds the copyright to Groundwork's MIT code, so code can move from Groundwork into Punchlist.
