# Split kvasir into an ingest daemon and a viewer over a local socket

kvasir runs as two processes: a headless, always-on **ingest daemon** that owns the
OTLP receiver, the data store, and the query/rollup engine; and a **SwiftUI viewer**
that renders dashboards. They communicate over a **local Unix-domain-socket typed
RPC** — request/response for queries plus a subscription channel for live updates —
not a shared database file and not a single combined process. The socket is
local-only in v1 (no network, no auth beyond filesystem permissions).

## Why

Harnesses push telemetry whenever they run, in terminals, all day — most of the time
no dashboard is open. Ingest therefore cannot depend on the UI being open, which
rules out a single combined app. Given two processes, a typed query API (rather than
a shared DB file) was chosen for a clean, versionable, typed boundary that keeps all
business logic in the Rust core and the SwiftUI layer logic-free, matching the
project's typed-payloads and minimal-UI-logic rules.

## Considered options

- **Single resident menu-bar app** (one process, receiver + UI together). Simplest,
  but ingest dies whenever the user quits the window; rejected.
- **Shared embedded DB as the contract** (daemon writes, viewer opens read-only).
  No protocol to maintain, but couples the viewer to the storage schema, makes the
  viewer's read path the writer's concern, and would have constrained the storage
  engine to one with strong multi-process concurrency (e.g. SQLite WAL). Rejected in
  favour of a typed API.
- **Networked API** (daemon on another machine). Deferred — no near-term remote use
  case, and it would add TLS/auth/exposure for a single-user local tool.

## Consequences

- The daemon is the **sole** process touching the store, which *reopens* analytical
  engines that dislike multi-process file access (e.g. DuckDB) — relevant to the
  storage-engine decision.
- A typed RPC schema becomes a real, maintained artifact (request/response enums,
  subscription events), serialized only at the socket boundary.
- Promoting to a networked daemon later is possible but is an explicit future change
  (transport, auth, security), not an accident of the current design.
