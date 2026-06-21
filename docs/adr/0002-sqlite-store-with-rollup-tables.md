# SQLite as the store, with normalize-on-ingest and rollup tables

kvasir's daemon stores telemetry in **SQLite** (via `rusqlite`). Raw OTLP is parsed
at the receiver into a typed canonical schema (one shape across all harnesses), and
kvasir maintains **rollup tables** (pre-aggregated per repo × model × measure × time
bucket) updated on ingest. Dashboards read the small rollup tables; raw detail tables
indexed by `session_id` / `prompt_id` serve trace drill-down.

## Why

The store lives inside an always-on background daemon, so idle footprint is a
standing cost. SQLite's page cache defaults to ~2 MB (incremental, hard-boundable
via `hard_heap_limit`), giving a single-digit-MB resident footprint. DuckDB defaults
to a memory_limit of 80% of RAM, needs ≥125 MB per worker thread for good operation,
runs a core-sized thread pool, and some operations escape its own memory limit —
realistically 100–300 MB+ resident, not cleanly bounded. At single-user volume
(low-single-digit millions of rows/year), SQLite with indexes and rollup tables
serves every planned view in sub-second time, so DuckDB's columnar advantage buys
nothing we need while costing resident RAM and a heavy native dependency to ship on
macOS/Windows/Linux.

## Considered options

- **DuckDB** — columnar, excellent for ad-hoc analytics; rejected on always-on-daemon
  memory footprint and shippable-binary weight versus no benefit at this volume.
- **A dedicated TSDB (Prometheus/VictoriaMetrics)** — separate server, models metrics
  but not logs/traces; wrong shape for one store over three signals. Rejected earlier.

## Consequences

- "Store every signal, roll up each measure from one authoritative signal" is
  realized as raw detail tables + maintained rollup tables; the rollup update path is
  where the authoritative-signal rule is enforced.
- If volume ever explodes (years of multi-machine history) or bulk ad-hoc analytics
  become a goal, the canonical schema makes swapping to DuckDB a migration, not a
  redesign. That swap is an explicit future decision, not designed in now.
- **Volume revised by the full-content (P3/A) decision:** capturing untruncated raw
  API bodies makes raw conversation/code blobs ~99% of stored bytes and pushes total
  volume to GB-scale, well past the lightweight estimate above. The lightweight
  metadata + rollup tables stay small and fast; the bulky blobs are compressed
  (zstd), encrypted at rest, and governed by a **shorter retention window** than the
  metadata. SQLite remains the engine — the dashboards still read small rollup/metadata
  tables — but blob storage, compaction, and retention become design concerns. See the
  forthcoming content-capture ADR.
