# Setup shim: auto-managed harness config plus a shell-hook for repo injection

kvasir configures the harnesses for the owner in two parts. **Static config** (enable
telemetry, all signals, Claude Code's trace beta, content gates, file-mode raw-body
directory, OTLP endpoint) is written **once** into each harness's persistent config:
Claude Code's `settings.json` `env` block, an `[otel]` table in Codex's
`~/.codex/config.toml` (required, because Codex ignores `OTEL_EXPORTER_OTLP_ENDPOINT`),
and the shell profile for Copilot. **Dynamic per-repo attribution** is injected by a
**shell hook** (`chpwd`/`precmd` in zsh, `PROMPT_COMMAND` in bash) that recomputes
`OTEL_RESOURCE_ATTRIBUTES="repo.name=…,repo.path=…"` from `git rev-parse
--show-toplevel` on every directory change. kvasir owns these edits: shown-before-write,
idempotent, in delimited kvasir-managed blocks, backed up, with a clean uninstall.

## Why

The three harnesses expose different configuration surfaces, so a single mechanism
can't configure all of them; static needs vs per-launch dynamic needs also differ.
A shell hook is the one **harness-agnostic** way to vary the repo attribute per launch
(it even reaches Codex, which honours `OTEL_RESOURCE_ATTRIBUTES` only via the OTel SDK
env detector). Auto-managing the config — rather than printing instructions — is the
difference between a product and a README, made safe by consent, backups, marked
blocks, and reversibility.

## Scope and consequences

- **Terminal-first coverage.** The hook fires only in interactive shells, so harnesses
  launched from IDEs/GUIs are not repo-attributed and fall into `<no-repo>`. Accepted:
  the owner launches harnesses from the terminal. A per-repo static-config complement
  (e.g. `.claude/settings.json` per repo) is the documented future path if IDE-launched
  attribution is ever needed.
- **Per-binary wrappers were rejected** in favour of the hook (universal, doesn't have
  to shadow each command).
- The shim is responsible for enabling the per-harness content gates behind the
  full-content decision (ADR-0003), to each harness's actual ceiling.
- Uninstall must restore every touched file from backup and remove managed blocks.
