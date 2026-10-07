# 4. Build the frontend as a web app

Date: 2026-10-07. Status: accepted.

## Context

ADR 0003 put the backend in Rust and left the frontend open. The candidates were a web app in TypeScript, React and Vite, and a native client in GPUI with GPUI Kit, whose WebAssembly build would serve links in a browser.

GPUI's strength is redrawing large, fast-changing views, as in Zed's editor. An issue tracker mostly draws lists of text, which a browser renders without trouble. A tracker feels fast because its data is local and its writes show before the server confirms them, which does not depend on the renderer.

Issue trackers also live on links: pull request comments, chat messages and notification emails all point at issues. GPUI's WebAssembly build draws to a single canvas, which usually costs native text selection, find-in-page and password managers, and adds a large first download. The verification pipeline is browser-based, and Playwright's role locators do not reach canvas elements. Agents also write React far more fluently than GPUI, which is pre-1.0 and changes with Zed's needs.

## Decision

- The frontend is a single-page web app in TypeScript, React and Vite. The Rust server serves its built assets.
- API types are generated from the Rust types, so the client and server cannot drift apart.
- Styling follows Groundwork's conventions: Tailwind with shadcn/ui, restyled through theme variables.
- Routing, data fetching and the approach to local data are chosen in the first slice that builds a screen.
- No desktop shell in v1. If one is needed later, Tauri wraps the same web app.

## Consequences

- Every issue has a URL that opens in any browser, with no install.
- Playwright and axe verify screens the same way they do in Groundwork.
- `punchlist-core` can compile to WebAssembly, so the client can validate a workflow file or preview a gate with the same code the server runs.
- The project uses two languages: Rust behind the API and TypeScript in front of it.
