# mlxtop user guide

[Back to the README](../README.md) · [Quick start](../README.md#try-it)

Detailed controls, runtime setup, and explanations of the dashboard readings.

## What it shows

| View | Question answered | Key information |
| --- | --- | --- |
| Overview | Is inference healthy, and how large is each request? | Per-request prompt chart and counts, generation/prefill rate, diagnosis, cache/queue, memory, paging/compression and GPU |
| MLX Top | Which process owns the workload? | PID, command, model, CPU, memory %, RSS, page-ins and serving state |
| Journal | What changed during this session? | Request lifecycle, provider/model changes, paging, pressure, compression, GPU, thermal and recovery events |

Overview embeds the compact prompt-load panel below its operational cards. Beneath
it, six time-series charts show generation, prefill and cache on the
first row; GPU, memory load and paging on the second. Each series keeps its own
unit and scale, so token rates are never visually mixed with percentages. Each
series is rendered as a stepped trace over a fixed-width tail of the ring
buffer: one terminal column represents one captured sample, new samples enter
from the right, and the oldest samples leave from the left once the viewport is
full. The generation and prefill axes use stable scales so a new peak cannot
move older values vertically. Rate traces use only active request telemetry,
and cache traces use the current sample's counter delta; cumulative session
averages are kept in the headline cards, not drawn as realtime samples.
Missing, stale, idle, or completion-log-only rate samples remain gaps, and the
chart title identifies the other active phase when a request is prefilling or
decoding.
A trace keeps the severity tone recorded with its sample, so a later refresh
cannot recolor or rewrite history. For readability, the plotted position uses
a causal deadband derived from the chart's drawable row resolution: movement of
at most a quarter-row is held at the prior plotted level, while larger changes
and spikes pass through immediately. The calculation uses only the samples up
to each point, so future samples never change an older plotted point. Raw values
remain the source for headlines and statistics. The tone is only a secondary
visual cue:
visible labels use meaningful states such as `normal`,
`watch`, `critical`, `loaded` and `saturated` rather than asking users to
interpret color names. On narrower terminals, the detailed cards collapse
into an operational strip instead of truncating critical data. Journal records
transitions rather than duplicating the live process table.

## Throughput correlation

When live provider telemetry reports a material generation-rate change,
mlxtop compares it with a short baseline for the same provider and model. It
then ranks signals observed in the same sample: paging, macOS memory pressure,
compression churn, thermal limiting, GPU saturation, Metal memory occupancy,
queue depth, model-memory growth and context/KV growth.

The Overview and MLX Top diagnosis cards keep the strongest evidence beside
the affected throughput, for example:

```text
GEN ↓16.7% (30.0→25.0 tok/s) · correlated: GPU 99% busy + context +11.7k → 32.2k
```

This is correlation, not a claim that one counter proves causation. If no
tracked system signal moved with the rate, the dashboard says so and points to
workload or runtime scheduling as the remaining explanation. A throughput
diagnosis is recorded once per change in the Journal, rather than once per
refresh.

## oMLX telemetry

The oMLX adapter checks `http://127.0.0.1:8080/health` by default and discovers
the configured host, port and optional API key from:

```text
~/.config/omlx-coding/server.env
```

When the local admin API is available, values are marked `LIVE`. If it is not,
mlxtop may use the latest structured completion record from these logs and
labels that fallback with its age:

```text
~/.omlx-coding/logs/server.log
~/.omlx-coding/logs/launchd.stdout.log
~/.omlx-coding/logs/launchd.stderr.log
```

API credentials are sent only to loopback endpoints by default. To use an
intentionally remote oMLX endpoint, set `MLXTOP_ALLOW_REMOTE_AUTH=1` and use a
trusted, protected network path.

## MLX and Metal telemetry

MLX allocator state belongs to the serving process. When an oMLX-compatible
endpoint exposes it, mlxtop displays the three counters separately:

- **active** — bytes currently held by live MLX arrays;
- **cache** — bytes retained by MLX's allocator pool; and
- **peak** — the process-local MLX peak since the runtime reset it.

The adapter also reads oMLX device/settings metadata when available, including
the MLX device name, architecture, unified-memory size, recommended working
set, maximum buffer size, process footprint and effective Metal limit. The
current model-memory and runtime-cache figures remain separate from allocator
memory so the dashboard does not double-count them.

On Apple Silicon, Metal telemetry is collected locally without sudo from
`ioreg`: device name, GPU core count, device/renderer/tiler utilization and
Metal system-memory allocation. `hw.machine` and `iogpu.wired_limit_mb` add
the architecture and explicit wired limit when the operating system exposes
them. The dashboard shows only allocator counters actually reported by the
provider; when none are available, it omits the allocator summary. The static
report retains `—` for missing fields. RSS, GPU load and model memory are never
substituted for allocator readings. mlxtop reads existing provider interfaces
and operating-system metrics; it does not patch or restart serving runtimes.

On macOS, the throughput card also shows **PROCESS <PID> · footprint**, followed
by **peak · growth · OS**. These values come directly from `proc_pid_rusage` for
the detected LLM process with the largest RSS. They describe that one process,
not the sum of all model servers; the PID identifies the scope.

- **Footprint** is the current OS-accounted physical footprint.
- **Peak** is the OS-reported lifetime maximum footprint, including activity
  before mlxtop started.
- **Growth** is the signed change in footprint per second between consecutive
  successful samples. It starts as `—` and resets after a missing sample, PID
  change or process restart. A positive value alone is not a memory alarm.

The static report includes that process's RSS as well. Footprint, RSS and MLX
allocator memory have different accounting; they are not interchangeable or
additive. OS process readings remain available while the server is idle and do
not require provider changes, restarts or administrator access for accessible
processes. Failed OS reads omit the process summary. This collector is macOS
only; Linux retains its existing metrics.

On Linux, GPU readings come from `nvidia-smi` when present (device name,
utilization, VRAM used/total and temperature); renderer/tiler splits and core
counts are Apple-only and stay unavailable. Thermals come from
`/sys/class/thermal` (plus the NVIDIA temperature when available).

## Controls

### Global

| Key | Action |
| --- | --- |
| `1` / `o` | Overview |
| `2` / `t` | MLX Top |
| `3` / `j` | Journal |
| `Tab` / `←` / `→` | Switch views |
| `p` / `Space` | Pause or resume sampling |
| `r` | Reset rates, charts and journal |
| `+` / `-` | Change refresh interval |
| `?` / `h` | Help |
| `q` / `Esc` / `Ctrl-C` | Quit |

### MLX Top

| Key | Action |
| --- | --- |
| `↑` / `↓` | Select a process |
| `PgUp` / `PgDn` | Move by ten processes |
| `Home` / `End` | First or last process |
| `s` | Cycle sort: RSS, CPU, PID, name |
| `f` / `/` | Enter a text filter |
| `c` | Clear the filter |

### Journal

| Key | Action |
| --- | --- |
| `↑` / `PgUp` / `Home` | Move toward newer events |
| `↓` / `PgDn` / `End` | Move toward older events |
| `f` / `]` | Next event filter |
| `[` | Previous event filter |

## Command-line options

```text
-i, --interval N   refresh interval in seconds, 1–60 (default: 1)
-n, --history N    chart/journal history, 20–3600 (default: 300)
-1, --once         print a static report and exit
-V, --version      print the installed version and exit
-h, --help         show help
```

## Crash diagnostics

Every run writes a small local diagnostic log to:

```text
~/Library/Logs/mlxtop/mlxtop.log        # macOS
~/.local/state/mlxtop/mlxtop.log        # Linux ($XDG_STATE_HOME respected)
```

Follow it while reproducing a crash:

```sh
tail -f ~/Library/Logs/mlxtop/mlxtop.log        # macOS
tail -f ~/.local/state/mlxtop/mlxtop.log        # Linux
```

Set `MLXTOP_LOG_PATH` to use another path. The log records session lifecycle,
sampling duration, provider/API availability, sampler failures, terminal
errors and panic backtraces. A normal shutdown ends with `event=process_exit`;
if that record is missing, the process was interrupted or crashed. The active
log is capped at 8 MiB and rotated once to `mlxtop.log.1`.

It contains counters and model/provider names for diagnosis, but never prompts,
model output, request bodies or provider API keys. Diagnostics are local-only
and are not uploaded.

## Request-token telemetry

The compact **prompt load** panel is built into **Overview**. It shows the
latest prompt size, the change from the previous observed request, and freshness
on a compact summary. At 160 columns and sufficient height, prompt load occupies
half of the first chart row beside generation and prefill, instead of a separate
full-width strip. It uses eight rows. Smaller terminals keep the stacked layout.
Token labels sit above
the bars so even small requests remain readable.
`LIVE` requires a matching request in a fresh live sample. Once absent
or stale it reads `LAST SEEN`; client-reported completions read `REPORTED`. Age comes from the request observation or the client timestamp, not the
most recent redraw. The last sampled output is not assumed to be a final total.

The colored bar chart reads older to newer, with the selected request marked
`▶` on the right. When request-specific cache counts are reported, bars stack
green cached tokens below uncached tokens (cyan for live requests, blue for
history). Without a cache count, a solid bar represents the whole prompt and
does not imply zero reuse. Yellow values and `!` mark a material prompt jump.
Segments are rounded to terminal-cell resolution; exact selected values remain
in the summary and cache line. Labels accompany color cues.

A jump means at least 25% and 2,048 more tokens than the previous same-model
observation. The insight area also compares against the median of up to eight
contiguous preceding requests from the same provider/model, after at least three
observations. At least 1.5× that median and 2,048 extra tokens is labeled large;
at most 0.75× is labeled smaller. These are workload comparison heuristics, not
context-limit or latency alarms. Historical assessments carry a **HISTORY**
label; their suggested checks are muted when the request is no longer live.
Cache availability is explicit, including when no request cache count was reported.

Request cache reuse is green at 80% or more, yellow below 20% for prompts of at
least 4,096 tokens, and cyan otherwise. Low reuse on a cold request is expected;
the hint to inspect prefix reuse is conditional on repeating prompts.

The chart uses a **fixed 0–65,536-token display scale**, independent of the visible
maximum. `↑` marks a request above that scale; the headline always gives its
exact size. This is a display scale, not a context limit or pressure threshold.
Cache counts appear only when reported for the selected request. Aggregate cache
statistics and prefill rates are never substituted for request-specific data.

Comparisons are labeled **PREVIOUS OBSERVED**, and are suppressed across changes
of provider or model. They do not establish conversation membership or explain
latency on their own. Request identifiers and sampled output counts remain in
Journal instead of occupying an Overview table.

Up to 240 distinct requests are retained. Repeated polls update an existing
observation, and past requests stay available. Use **↑ / ↓** or **PgUp / PgDn**
to browse, **Home** for latest, **End** for oldest, and **r** to reset history.

## Operator charts

On wide terminals, the second chart row contains **process memory**, **queue**,
**GPU** and **system memory**. Paging and aggregate cache remain below. This
keeps process footprint separate from overall system load and retains the
existing throughput and hardware charts.

**Queue** plots active requests in cyan and waiting requests in yellow on one
shared, labeled zero baseline. A white `═` marks overlapping trace cells,
including equal values and values that coincide at terminal resolution. Exact
counts remain in the header. Both series use a fixed scale of 0–16 requests,
with `↑` for overflow. Idle zeros are valid; stale, missing and client-reported values
produce gaps. Queue length is a demand signal, not a latency measurement.

**Process memory** plots the OS physical footprint against a fixed display
scale of total system RAM. The header identifies the current PID. This RAM
reference is not the process's configured memory limit. Missing samples and
process-instance changes break the trace. Both new time-series charts retain
one captured sample per column, newest at the right; resetting history clears
them. The process card still shows lifetime peak and signed growth.

**First token** appears only after an explicit client timing is supplied in the
existing usage JSONL envelope:

```json
"timings": {"time_to_first_token_ms": 1250}
```

Use a nonnegative integer measured from request dispatch to the first generated
token. Add this field alongside `provider`, `request_id`, `observed_at` and
`usage` in a complete record. The chart uses one column per observed request,
shows missing timings as gaps, and has a fixed 0–30-second scale with overflow
markers. Its latest measured value is labeled REPORTED with observation age.
Repeated polls update the same request rather than adding duplicate bars.
Prompt-evaluation duration, generation speed and polling intervals are never
used to estimate first-token latency. No provider changes are required.

When first-token data is available, that chart occupies the wide grid's recent
Journal preview space; the full Journal remains accessible in its tab. Without
timing data the preview remains, so no empty latency panel consumes space.


Overview shows `PROMPT` for the selected request, alongside the provider and
telemetry source. The static report includes prompt/output counts and individual
request observations. Journal records newly observed request IDs and prompt
counts across oMLX models, including concurrent requests. Equal-sized requests
remain separate; repeated polls of the same request/count do not create entries.
The journal retains a bounded deduplication window of 512 observations.

`observed` means a sampled active request. `reported` means a completed request
reported by a provider or client. Cached tokens are shown only when supplied for
that request; the aggregate CACHE percentage is never substituted. Counts and
rates from completed requests are historical and do not enter live rate charts.
The age on KoboldCpp results is time since first observed, because its performance
endpoint does not provide the completion timestamp.

Automatic selection follows the detected provider. To select a particular local
server, use one of these commands:

```sh
MLXTOP_PROVIDER=koboldcpp ./target/release/mlxtop
MLXTOP_PROVIDER=llama.cpp MLXTOP_PROVIDER_PORT=8081 ./target/release/mlxtop
```

Native adapters use loopback only: KoboldCpp defaults to port 5001 and
llama-server to port 8080. `MLXTOP_PROVIDER_PORT` overrides those ports. No API
keys are sent by these adapters. oMLX retains its existing endpoint and login
configuration. Explicit provider selection takes priority over process detection.
The dashboard selects one provider; it does not merge unrelated servers.

- **oMLX:** reads active request IDs and prompt counts from admin statistics.
  Some engines/phases do not expose counts; untokenized queued zeros are skipped
  in the request journal.
- **KoboldCpp:** reads `/api/extra/perf` last-result counts and rates. This is not
  a complete request history, and a new active request does not make the previous
  completion's rates live.
- **llama-server:** reads `/slots`; optional `/metrics` supplies aggregate average
  rates and active/deferred queue counts when the server starts with `--metrics`.
  Either endpoint can work independently. Output counts sum all active slots;
  if any active slot omits its output count, the total stays unavailable.
  Slot capacity, processed prefill
  work and retained context are not treated as full request prompt counts. Use
  the usage-file integration below for exact completion usage.
- **MLX-LM, Ollama, LM Studio and LocalAI:** their response usage needs client
  integration. Without it, prompt counts remain unavailable. LocalAI's aggregate
  usage API is not treated as individual requests.

### Client-reported usage file

A client can append one counters-only JSON object per completed request to a
local JSONL file, then launch mlxtop with:

```sh
MLXTOP_USAGE_FILE=/absolute/path/usage.jsonl ./target/release/mlxtop
```

For llama-server, the file supplements native polling: completed requests appear
in prompt history while live slots and queue counts remain available. Completed
prompt counts are never combined with an active slot's output to estimate context.
For other runtimes, valid file records take priority over native last-result
reports (and over oMLX polling). An empty or invalid file does not disable
llama-server or KoboldCpp polling.

mlxtop only reads the file; your client must write the records. Explicit or
detected provider selection filters the file to that runtime. If no runtime is
selected, the newest valid record selects the provider for that refresh.
Each record requires `provider`, a unique `request_id`,
`observed_at` (completion time as Unix seconds), and token counts. `model` is
optional. Example shape (replace the timestamp with the completion time):

```json
{"provider":"mlx-lm","request_id":"req-123","model":"local-model","observed_at":1700000000,"usage":{"prompt_tokens":32768,"completion_tokens":120,"prompt_tokens_details":{"cached_tokens":24576}}}
```

Supported provider names: `omlx`, `mlx-lm` (also `mlx_lm.server`), `ollama`,
`llama.cpp` (also `llama-server`), `lmstudio` (also `LM Studio`), `koboldcpp`,
and `localai`.

Copy only usage counters from the response, with the required envelope above:

| Response format | Counter fields |
| --- | --- |
| OpenAI-compatible usage, including MLX-LM | `usage.prompt_tokens`, `usage.completion_tokens`, optional `usage.prompt_tokens_details.cached_tokens` |
| Responses-style usage | `usage.input_tokens`, `usage.output_tokens`, optional `usage.input_tokens_details.cached_tokens` |
| Ollama native | `prompt_eval_count`, `eval_count` at record top level |
| LM Studio native | `stats.input_tokens`, `stats.total_output_tokens`; `model_instance_id` is accepted as the model |

For MLX-LM streaming, request `stream_options: {"include_usage": true}` and copy
the final usage chunk. Other streaming APIs may likewise require usage to be
enabled. Do not include prompts, messages, generated text, request bodies or keys
in this file. Use opaque request IDs, unique across server restarts.

The repository includes a standard-library Python helper, `scripts/record_usage.py`.
Call it from your existing client after a completion:

```python
from scripts.record_usage import append_usage

# response is the final response dictionary from your existing API call.
append_usage("/absolute/path/usage.jsonl", "ollama", response)
```

It accepts the response formats in the table for every listed provider. For SDK
objects, pass their dictionary representation (for example, `model_dump()`).
For streaming, pass only the final usage-bearing chunk, or LM Studio's final
aggregated response. The helper generates a unique request ID and completion
timestamp; pass `request_id`, `observed_at`, `model` or measured `ttft_ms` explicitly
when needed. When importing a saved response, supply its original completion
time rather than treating the import time as the completion time.

Alternatively, pipe a completed JSON response to the helper:

```sh
python3 scripts/record_usage.py --provider mlx-lm \
  --file /absolute/path/usage.jsonl < completed-response.json
MLXTOP_PROVIDER=mlx-lm MLXTOP_USAGE_FILE=/absolute/path/usage.jsonl mlxtop
```

The helper writes only allowlisted identifiers, counters and explicit timing.
It excludes messages, generated content, reasoning, tool results and keys. New
files have mode `0600`; cooperating writers use a file lock. It neither proxies
requests nor changes a serving runtime. Python is needed only for this optional
helper, not for mlxtop itself.

Only newline-terminated records are read. Invalid records are skipped, missing
counts remain unavailable, and future timestamps are rejected. Reading is bounded
to the last 256 KiB and at most 128 distinct recent requests for the selected
provider. Duplicate provider/model/request IDs retain their last file record.
The original timestamp
is retained on every refresh; polling an old file does not make its data live.
To retain every completion, the client should keep its own usage history. mlxtop's
sampled journal is not an accounting ledger.

Prompt differences are not labeled as conversation growth: provider request IDs
do not establish that successive requests belong to the same tool loop.

### Runtime detection and endpoint references

Process detection includes Python's `-m mlx_lm.server`, `KoboldCpp.py`, `local-ai`,
LM Studio desktop/engine paths, its `llmster` daemon, and the Bionic app executable
(`Bionic.app/Contents/MacOS/Bionic`). Bionic is labeled as LM Studio. If several
runtimes are running, use `MLXTOP_PROVIDER` to choose which provider supplies telemetry.
Detection alone does not expose request tokens from process memory.

The adapters follow the upstream [llama-server monitoring API](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
and [KoboldCpp performance endpoint](https://github.com/LostRuins/koboldcpp/blob/concedo/koboldcpp.py).
Client formats are documented by [MLX-LM](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/SERVER.md),
[Ollama](https://docs.ollama.com/api/usage), and
[LM Studio](https://lmstudio.ai/docs/developer/rest/chat).
Native adapters target a single loopback server, without authentication;
llama-server router mode and authenticated monitoring endpoints are not supported.

## Data sources and privacy

mlxtop reads macOS counters from `sysctl`, `memory_pressure`, `vm_stat`,
`ioreg`, `pmset` and `ps`. On Linux it reads `/proc/meminfo`, `/proc/vmstat`,
`/proc/pressure/memory`, `/sys/class/thermal`, `nvidia-smi` (when present)
and `ps`. It reads local provider endpoints and logs only for
supported adapters. The application does not contain analytics, upload
collected metrics, modify model state or send synthetic inference requests.

Provider credentials are read only when needed and are never displayed. See
[SECURITY.md](../SECURITY.md) for responsible vulnerability reporting and the
remote-authentication boundary.

## Development

Run the same checks used by CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```

For UI changes, test both the interactive dashboard and `mlxtop --once` and
include a terminal screenshot with the pull request. See
[CONTRIBUTING.md](../CONTRIBUTING.md) for design boundaries and contribution
licensing. The longer-term provider and architecture plan is documented in
[OPEN_SOURCE_ROADMAP.md](https://github.com/maximpri/mlxtop/blob/main/OPEN_SOURCE_ROADMAP.md).

## Deployment helper

The optional deployment script creates versioned remote releases and
updates a `current` symlink. A binary-only deployment sends only the locally
built executable:

```sh
cargo build --release --locked
BUILD_ON_REMOTE=0 ./scripts/cicd.sh --host example.local --user deploy
```

Useful overrides:

```text
REMOTE_DIR                 remote install root (default: $HOME/mlxtop)
SSH_KEY                    private key for SSH/SCP
BUILD_ON_REMOTE            1 to build on target, 0 for binary-only install
KEEP_RELEASES              release count to retain
RESTART_COMMAND            optional restart hook
HEALTHCHECK_COMMAND        optional post-deploy check
```

## Build a macOS disk image

Start with a release payload containing the `mlxtop` binary, documentation,
`LICENSE`, `THIRD_PARTY_NOTICES.md`, and a `licenses` directory with the dependency
and Rust standard-library notices. Package it on macOS:

```sh
./scripts/package-dmg.sh path/to/release-payload target/dmg-release
```

This creates a compressed DMG containing `Install mlxtop.pkg` and installation
instructions, plus a `SHA256SUMS` file beside the DMG. The package installs the
command in `/usr/local/bin` and documentation and notices under
`/usr/local/share/mlxtop/<version>`. It requires an administrator account.
The build script creates an unsigned package and does not notarize it.

The Terminal installer in `scripts/install.sh` downloads the same DMG, verifies
its checksum, and extracts the package to install the command in `~/.local/bin`
without sudo. Use the installer linked from the current README; the original
installer in the historical v1.0.0 source tag used the superseded tarball.
