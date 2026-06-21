# kvasir

A native desktop application that ingests OpenTelemetry data emitted by AI coding
tools and presents usage analytics — tokens, cost, and tool activity — broken down
along dimensions a developer cares about (repo, model, time).

## Language

**Harness**:
An AI coding tool that a developer runs locally and whose usage kvasir ingests
(e.g. Claude Code, GitHub Copilot CLI, Codex CLI, OpenCode). kvasir consumes a
harness's telemetry; it does not run or wrap the model itself. Most harnesses report
via OpenTelemetry, but not all (Antigravity CLI exposes only local hooks/files), and
those that do don't all emit every signal — OpenCode, for instance, emits traces and
logs but no metrics. The ingest mechanism and signal coverage are properties of each
harness, not a guarantee.
_Avoid_: Agent, CLI, client, tool (overloaded — "tool" means a harness's tool call).

**Session**:
One run of a harness, identified by the harness's own session/conversation id.
The natural grain at which usage is reported before kvasir rolls it up by repo,
model, or time.
_Avoid_: Run, conversation, thread (these are harness-specific spellings of the
same concept).

**Cost**:
The monetary value of usage, in USD. Only some harnesses report it directly
(Claude Code does); for the rest kvasir derives it from token counts and a
per-model price table it maintains. A derived cost is therefore an estimate, not
a billed amount.
_Avoid_: Price, spend, bill.

**Repo**:
The unit kvasir attributes usage to. Defined as the git top-level directory
(`git rev-parse --show-toplevel`) in effect when a harness session is launched.
Sessions launched outside any git repo fall into a single explicit no-repo bucket.
The harness telemetry does not carry this natively; kvasir establishes it by
injecting a resource attribute at launch.
_Avoid_: Project, folder, workspace, directory.

**Tool call**:
A single invocation of one of a harness's tools (Read, Edit, Bash, an MCP tool,
etc.) during a session, as reported by the harness's telemetry.
_Avoid_: Action, command, function call.

### Signals

**Signal**:
One of the three OpenTelemetry data types kvasir ingests: **metrics**, **logs**,
**traces**. kvasir consumes all three. A given fact (e.g. a token count) may appear
in more than one signal; that overlap is a normalization concern, not three
separate facts.
_Avoid_: Stream, channel, feed.

**Event**:
A single log record on the logs signal (e.g. a harness's `tool_result` or
`user_prompt`). An event is a kind of log, not a fourth signal alongside
metrics/logs/traces.
_Avoid_: Log line, message (when the OTLP log-record meaning is intended).

**Measure**:
A quantity kvasir rolls up — input tokens, output tokens, cost, tool-call count.
Because a measure can be reported by several signals at once, each measure has one
**authoritative signal** that rollups read from; the others are kept for detail and
cross-checking but never summed alongside it.
_Avoid_: Metric (reserve "metric" for the OTLP metrics signal specifically).

**Dimension**:
An axis a measure is sliced by. **Core dimensions** (time, harness, repo, model,
session, token type, tool name) are present for every harness and drive the
cross-harness views. **Extended dimensions** (subagent, skill, plugin, mcp server/tool,
effort, speed, query source, account/org) are populated only when a harness emits them
— mostly Claude Code — and surface in the UI where present, absent otherwise.
_Avoid_: Facet, attribute, tag, label (use "dimension" for the kvasir-canonical axis).
