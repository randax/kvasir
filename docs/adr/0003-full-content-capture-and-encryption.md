# Capture full harness content, and encrypt the store at rest

kvasir captures **full content** from every harness: prompt text, tool inputs and
outputs, and **untruncated raw API request/response bodies** (verbatim model
responses and full per-request context). Because that turns the store into a
sensitive corpus of conversations and source code, the SQLite database is
**encrypted at rest with SQLCipher (AES-256)**, keyed by a random 256-bit key held in
the OS keychain and unlocked transparently at daemon startup.

## Why

The owner wants verbatim model responses and full-context replay, not just usage
counts — so metadata-only capture was rejected. Full content makes encryption
non-optional: a plaintext archive of everything run through three harnesses is too
dangerous to leave unprotected.

## What this entails

- **Two ingest paths in the daemon.** The OTLP receiver (metrics/logs/traces) *plus* a
  **body-file importer**: raw bodies are captured in file-mode
  (`OTEL_LOG_RAW_API_BODIES=file:<dir>`) to avoid the 60 KB inline truncation that
  would defeat full fidelity. The importer reads each body file, compresses it
  (zstd), stores it encrypted, then securely deletes the source file.
- **kvasir owns the body directory.** Each harness writes untruncated bodies as
  plaintext files *before* kvasir ingests them; pointing that directory at a
  kvasir-owned location and shredding files immediately after import shrinks (does not
  eliminate) the plaintext-on-disk window.
- **Whole-DB encryption, not field-level.** One key protects everything; avoids
  drawing a sensitivity line through the schema. `rusqlite` with bundled SQLCipher.
- **Key in the OS keychain via the `keyring` crate** — macOS Keychain, Windows
  Credential Manager/DPAPI, Linux Secret Service (file fallback at `0600`). Same Rust
  code, three backends.
- **Transparent unlock.** The headless always-on daemon reads the key automatically
  at startup; a user passphrase was rejected because it would block unattended
  startup and contradict the resident-daemon design (ADR-0001).

## Threat model (explicit)

- **Defends against:** disk theft, drive images, backups, another OS user, casual
  snooping.
- **Does NOT defend against:** malware running as the same OS user, which can request
  the same key from the keychain. No transparent-unlock local app can; a passphrase
  could, only by sacrificing unattended operation. This trade-off is accepted.

## Consequences

- Raw bodies are ~99% of stored bytes and grow O(N²) within a session, so a **tiered
  retention** policy (short window for bodies, indefinite for lightweight
  metadata/rollups) is required — see the retention decision and ADR-0002's revised
  volume note.
- Enabling all content gates is a responsibility of the setup shim, per harness.
