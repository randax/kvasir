## Plan

Implement #16 with vertical TDD slices:

1. Import an untruncated Claude file-mode raw API body from the kvasir-owned body directory.
2. Store the imported body compressed and encrypted, linked to the request/session content path.
3. Remove the source body file after a successful import.
4. Expose replay retrieval through the existing RPC/client path without leaking plaintext into storage.

## Verification

- Add behavior-first tests for each slice before implementation.
- Run focused Rust tests as each slice goes green.
- Run final formatting, clippy with `-D warnings`, and the relevant test suites.

## Notes

- No new dependencies unless already available in the workspace.
- Keep structured protocol data typed and only serialize at I/O boundaries.
