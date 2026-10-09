# mlxtop user guide

[Back to the README](../README.md) · [Quick start](../README.md#try-it)

Detailed controls, runtime setup, and explanations of the dashboard readings.

## What it shows

| View | Question answered | Key information |
| --- | --- | --- |
| Overview | Is memory or paging limiting inference? | Memory pressure and paging, readable throughput, GPU, prompt history, cache, queue and recent events |
| MLX Top | Which process owns the workload? | PID, command, CPU, memory %, RSS, page-ins, OS state and selected runtime details |
| Journal | What changed during this session? | Request lifecycle, provider/model changes, paging, pressure, compression, GPU, thermal and recovery events |

Overview starts with a dense full-width **SYSINFO** strip and a fixed two-row
assessment: the current finding and a suggested check or neutral observation. **Memory / pressure**
and **paging** share a compact first row. **Generation**, **prefill** and **GPU**
use the next row, with enough height to show rate changes clearly. Select a
chart and press Enter for its full view. OS process footprint is available in
MLX Top and the static report; Overview has no process-memory chart.
System memory shows **resident** physical RAM, its byte count/total, and
**Includes file cache**. This is occupied RAM, including reclaimable cache.
On macOS it is total RAM minus free and speculative pages; on Linux it is
`MemTotal - MemFree`. It is independent of **PRESSURE**, which retains OS severity.
Its reading, trace and compact gauge use cyan for occupancy. The separate
PRESSURE label uses green for normal, yellow for watch, red for critical and
muted for unknown.
A full cache alone does not establish memory pressure. Linux derives pressure
from unavailable memory (`MemTotal - MemAvailable`) and full PSI stalls;
`memory_warn_load` and `memory_critical_load` configure those Linux bands.
macOS uses native pressure rather than percentage thresholds. macOS `memory_pressure -Q` includes pageable
application pages in its reported percentage and is not used as RAM usage.

A compact row holds **prompt load**, **Cache** and **Queue**. The bottom **recent
Journal** shows timestamped changes, newest first. At 170×42 the resource row is
eight rows high, throughput twelve and Journal has three event rows, with long messages
wrapping to a second line. Press `3` for the full Journal.

Prompt load uses nine to thirteen rows on regular terminals. Each panel keeps its place
when requests arrive or the model becomes idle. Each chart has one border and
independent selection and expansion controls. Time-series panels share zoom;
request bars retain independent zoom. Hardware, RAM, CPU/RSS,
thermal and GPU allocation live in SYSINFO alongside model and telemetry age.
The top bar holds view navigation and sampling state; the bottom bar shows
controls for the current view and selected chart. Detailed chart statistics
appear in expanded views.

**SWAP** is a horizontal used/total capacity bar inside paging / I/O. Its
percentage describes occupied capacity; the separate B/s history describes
paging traffic. A measured all-zero window says **No paging traffic** and keeps
its zero trace without inventing a 1 B/s ceiling. Missing samples remain gaps.
On short terminals, percentage panels show a capacity bar and
exact reading; Enter opens the full history. If only aggregate cache reuse is
available, Cache retains its interval empty state and TOTAL/prefix hit text.
It never switches to a cumulative gauge.
Idle models can show the latest observed prompt with its age. Selected prompt
history identifies a different provider/model when browsing retained requests.

Only percentage charts use a 0–100 axis. Numeric charts automatically fit the
visible observations in their actual units: tok/s, tokens, bytes, bytes/s,
requests or milliseconds. Generation and prefill have independent ranges, so
a model running at 35 tok/s is not forced onto a 100 tok/s ceiling. Zooming the
history recalculates these ranges. The current axis is labeled; its limits are
display ranges, not hardware or model limits. See the
[chart specification](CHART_SPEC.md) for the rules shared by every chart.

Each time series is a stepped trace, newest at the right. Overview uses the
same trailing sample window for every time-series panel. Wider plots widen
those same observations; no samples are dropped. Window labels describe
sample slots at the selected sampling cadence. Expanded charts show more history. Rate traces contain only active-request
telemetry. Server averages and retained results stay explicitly labeled
`AVG` or `LAST`; they are never plotted as live samples. Missing, stale, idle
and completion-log-only rate samples leave gaps. A provider/model change
starts a new throughput history.
An isolated observation appears as a dot. When recent samples are missing, the
chart labels the last visible reading and its age. `window avg` summarizes the
visible captured samples; `SERVER AVG` in expanded views is the separate
provider average, which may predate mlxtop. Overview prioritizes LIVE and LAST.
Cache labels interval samples independently from its cumulative `TOTAL` reuse.
oMLX's initial prefill speed placeholder leaves a gap until a measured rate is
available. A short prefill can finish between polls without producing a live
rate sample; its server average remains available in the expanded chart. Short Queue panels show exact
counts; Enter expands their history with a readable request scale.
A trace keeps the severity tone recorded with its sample, so a later refresh
cannot recolor or rewrite history. For readability, the plotted position uses
a causal deadband derived from the chart's drawable row resolution: movement of
at most a quarter-row is held at the prior plotted level, while larger changes
and spikes pass through immediately. The calculation uses only the samples up
to each point, so future samples never change an older plotted point. Raw values
remain the source for headlines and statistics. The tone is only a secondary
visual cue:
visible labels use meaningful states such as `normal`,
`watch`, `critical`, `busy` and `saturated` rather than asking users to
interpret color names. On narrower terminals, the detailed cards collapse
by keeping numeric readings when a secondary trace cannot fit. Journal records
transitions rather than duplicating the live process table.

## Throughput correlation

When live provider telemetry reports a material generation-rate change,
mlxtop compares it with a short baseline for the same provider and model. It
then ranks signals observed in the same sample: paging, macOS memory pressure,
compression churn, thermal limiting, GPU saturation, Metal memory occupancy,
queue depth, model-memory growth and context/KV growth.

The Journal records the strongest evidence with the affected throughput,
for example:

```text
GEN ↓16.7% (30.0→25.0 tok/s) · correlated: GPU 99% busy + context +11.7k → 32.2k
```

This is correlation, not a claim that one counter proves causation. If no
tracked system signal moved with the rate, the Journal finding says so and identifies workload or runtime scheduling
as a possible explanation. Diagnostics cards have been removed. A throughput
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

On macOS, MLX Top and the static report show OS process memory. Values come
directly from `proc_pid_rusage` for the detected LLM process with the largest
RSS. They describe that one process, not the sum of all model servers; the PID
identifies the scope.

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

The NVIDIA dashboard is enabled only on Linux after `nvidia-smi` detects at
least one NVIDIA GPU. macOS and Linux systems without detected NVIDIA cards
use the standard Overview. Overview's **GPU
DEVICES** panel lists every detected NVIDIA card with its driver index, name,
utilization, VRAM used/total and temperature. Wide terminals add a VRAM bar
and a textual load state. Use `[` / `]` to select a card; when the list is
taller than the panel, the selected card scrolls into view and the footer
shows the visible range. At 80×24, status, throughput, prompt sizes, GPU readings
and prompt history remain visible; historical charts return with more space.

Cards are tracked by UUID, following [NVIDIA's guidance on stable device
identity](https://docs.nvidia.com/deploy/nvidia-smi/), so identical model names remain distinct and a
selection follows the same device if driver indices change. The **GPU max**
chart and summary show the highest utilization across cards, not an average.
If any card's utilization is unavailable, the combined reading stays
unavailable while the other per-card readings remain visible. A failed poll
retains known device names but clears their counters until a successful poll.
Unsupported counters appear as `—`; idle utilization is a measured `0%`.

The v1.2 example below uses simulated readings for four NVIDIA cards. It shows
the per-card fields; the current Overview layout is described above.

![v1.2 Overview example with four simulated NVIDIA cards and per-card utilization and VRAM](screenshots/nvidia-multi-gpu.png)

VRAM is shown separately for each card; any **VRAM sum** in the summary is an
inventory total, not a shared allocation pool. `--once` lists all cards and
their UUIDs without pagination. These are physical-device readings, not
per-process measurements or a breakdown of MIG instances. Renderer/tiler splits
and core counts are Apple-only and stay unavailable. Thermals come from
`/sys/class/thermal` (plus the NVIDIA temperature when available).

## Controls

### Global

| Key | Action |
| --- | --- |
| `1` / `o` | Overview |
| `2` / `t` | MLX Top |
| `3` / `j` | Journal |
| `Tab` / `Shift-Tab` | Next / previous view, including while filtering processes |
| `p` / `Space` | Pause or resume sampling |
| `r` | Reset rates, charts and journal |
| `{` / `}` | Change refresh interval (1–60 seconds) |
| `a` | Acknowledge the current critical system alarm |
| `?` / `h` | Help |
| `q` / `Ctrl-C` | Quit (`Esc` restores an enlarged chart before quitting) |

### Diagnostics and runtime setup

Press `d` from any main view to open **Diagnostics**. During process-filter entry,
`d` remains a filter character; finish the filter first. The panel shows the full
assessment, supporting evidence, and confidence when a measured slowdown has a
correlated signal. Local hardware findings are explicitly labeled when the selected
API is remote; they do not establish a remote inference bottleneck.

The connection section shows selection provenance, the resolved endpoint, hidden
credential status, polling ages, endpoint failures and capability sources. A
connected inventory API does not establish live throughput. Native live rates,
server averages, last completions, and client-reported usage retain their separate
meanings. A missing reading stays unavailable. Optional endpoint failures can
leave a connection partially available; legacy LM Studio fallbacks remain usable.

Use arrows, Page Up/Down, Home/End, or the mouse wheel to scroll. `d` or Esc closes
the panel. Tab/Shift-Tab and 1/2/3 close it and switch views. `p` pauses sampling,
`a` acknowledges a critical alarm, and `q` or Ctrl-C quits. Opening the panel does
not make additional API requests or edit configuration.

```sh
mlxtop doctor
mlxtop doctor --interval 2
MLXTOP_PROVIDER=ollama mlxtop doctor
```

The doctor command uses the same report and configuration precedence. It samples
twice, honors the refresh interval, and needs no TTY. Exit code 1 means invalid
configuration, incomplete host counters, or an unusable selected primary API;
exit code 0 includes no detected runtime and partial connections with missing
optional measurements. Recorder-only collection is labeled separately and does
not claim an API connection. High pressure is reported without making the command fail.
Unlike the dashboard's tolerant startup, doctor rejects malformed configuration
and out-of-range interval/history values. `doctor` cannot be combined with `--once`.

Setup guidance explains API activation and recorder requirements. Integrate the
[client recorder](#client-reported-usage-file) in the calling client before setting
`MLXTOP_USAGE_FILE`; the environment variable alone does not record requests.
Credentials and raw API responses are excluded from the report.

![Read-only Diagnostics panel with runtime capabilities](screenshots/diagnostics.png)

On compact terminals, the two-row assessment uses the Overview Journal preview's
space. The complete Journal remains accessible with `3`.

### Overview charts

| Control | Action |
| --- | --- |
| Arrow keys | Select a neighboring chart |
| Mouse click | Select the chart under the pointer |
| `+` / `-` or mouse wheel | Zoom time series together, or selected request bars, from 1× to 8× |
| `0` | Reset time-series zoom, or selected request bars, to 1× |
| `Enter` / `Esc` | Enlarge / restore the selected chart |
| Right-click | Toggle enlarged view |
| `Shift-↑` / `Shift-↓` | Inspect newer / older prompt observations |
| `Home` / `End` | Select newest / oldest prompt |
| `PgUp` / `PgDn` | Move through prompt history by ten requests |
| `[` / `]` | Select an NVIDIA card on Linux |

Time-series charts share zoom. Prompt and latency bars retain their own zoom. Zoom widens captured observations and shows a
shorter history range; it does not change the sampling interval or the values.
Expanded charts keep sampling, and Enter/Esc restores the dashboard. Hold the
terminal's selection modifier (usually Shift) to select text with the mouse.

### Critical system alarms

Critical memory pressure, heavy paging, swap thrashing and page-in recovery
ring the terminal bell once and display a banner. In Overview the banner takes
the place of the assessment rows, so SYSINFO's model, state and data age stay
visible. Enable the audible bell in
your terminal settings to hear it. `a` dismisses the banner for the rest of
that episode; a confirmed recovery re-arms the alarm. Missing system counters
do not count as recovery. High GPU utilization is normal workload activity
and never triggers this alarm by itself.

Diagnostics distinguish measured throughput drops from resource usage. When
nothing needs action and memory pressure is normal, the assessment reads
**Healthy · no bottleneck**; GPU utilization appears as evidence beside it, and
a busy GPU alone never becomes the finding. A measured drop shows the rate
change, an associated signal with confidence, and a short suggested check.
Linux temperature readings are shown as measurements, not inferred throttling.

### MLX Top

The process table fills the view, with aggregate CPU/RSS for the current filter.
Select a row to see its full command, OS state, CPU, RAM share, RSS and page-ins
per second. OS footprint/peak/growth appear only for their measured PID.
Provider-wide model/state telemetry is labeled **RUNTIME** in the details;
it is never assigned to every process row. Remote telemetry does not imply
that a corresponding process exists on the machine running mlxtop.

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

The **prompt load** panel in **Overview** shows **prompt size in input tokens**.
The headline is the selected request's exact input-token count, including cached
tokens. For example, a 20,000-token prompt with 15,000 cached tokens still shows
**20,000 tokens**; its uncached portion is 5,000 tokens. Each bar represents one
observed request, and selecting an older request updates the headline to that
request's size.

The summary also shows the change from the previous observed request and the
observation's freshness. Prompt load uses half the width of a compact row,
beside Cache and Queue. It is nine to thirteen rows high, or seven on short macOS terminals
and six with a compact NVIDIA device table. Enter expands it for a larger view.
Newest requests stay at the right with fixed spacing, including a single bar.
Every visible bar has a size label beneath it,
such as `12.0k` for 12,000 tokens (`k` means thousands, `M` means millions;
these are token counts, not kilobytes). Each request gets enough horizontal
space to keep its label readable, including at 1×. Older requests remain
available with Shift+↑↓ and Home/End. Exact selected
tokens, observation time in UTC, age, cache reuse and the previous-request
comparison stay above the chart; recent prompt-size statistics sit in the
bottom border.
`LIVE` requires a matching request in a fresh live sample. Once absent or stale,
an incomplete request reads `LAST SEEN`; completed request counts read `REPORTED`.
Age comes from the request observation or the client timestamp, not the most
recent redraw. The last sampled output is not assumed to be a final total.

Each selected prompt also shows its output count and output speed, for example
**OUT 346 · LAST 40.0 tok/s**. `LIVE` is the active request's reported decode
speed; `AVG` is a completed request's reported average; `LAST` is its last
measured rate when no final timing is available. Use Shift+↑↓ to compare speeds
for earlier prompts. Missing request timing shows `SPEED —`; the server average,
prefill rate and other concurrent requests are never used as substitutes.
A later observation without timing retains the previous speed and its original
age, labeled `LAST`. Request speed uses the existing prompt metadata rows;
small panels prioritize it over the UTC clock and optional comparison details.

Prompt counts, output-speed observations, chart samples and Journal events are
kept in memory for this mlxtop session. After a restart, the prompt panel waits
for new requests; the server's aggregate averages may still be available.

For oMLX this is each generating row's `tokens_per_second`, or distributed
per-request `decode_tps`. These rates cover output generation. Once an oMLX
request disappears between polls, the retained sample is not a final average.
KoboldCpp supplies `last_eval_speed`. A completed Ollama usage record can supply
`eval_count` and `eval_duration` (nanoseconds); their ratio yields output tok/s.
Total request duration and prompt-evaluation duration are not decode timing.


The colored bar chart reads older to newer, with the selected request marked
`▲` beside the size label below the rightmost bar. A single request starts at
the right edge; fixed spacing keeps it there as more requests arrive. Its UTC timestamp stays in
the headline. Spacing represents request order, not elapsed time; the selected marker
remains visible during jumps and overflow. When request-specific cache counts are reported, bars stack
green cached tokens below uncached tokens (cyan for live requests, blue for
history). Without a cache count, a solid bar represents the whole prompt and
does not imply zero reuse. `↑` marks a prompt above the display scale. Changes
in prompt size remain numeric comparisons, without warning marks above bars.
Bar heights use eighth-cell precision and do not change with cache availability.
Where a partial cell cannot show both segments and empty space, it uses the
dominant segment color. Exact selected values remain in the summary and cache
readout. Labels accompany color cues.

The bottom line shows the median and range of up to eight contiguous
observations ending at the selection, from the same provider/model. Moving
back through history excludes later requests from those statistics. These are
prompt sizes in tokens, not health or latency assessments. The headline keeps
the exact selected prompt size and its observation age visible.

Request cache reuse is green at 80% or more, yellow below 20% for prompts of at
least 4,096 tokens, and cyan otherwise. Low reuse on a cold request is expected;
a low percentage by itself does not establish a cache problem.

The chart uses an **automatic zero-based token scale** derived from the visible
requests. A history of roughly 800-token prompts therefore fills a useful
range near 1,000 tokens. Larger prompts and history zoom update that range;
the headline always gives the selected request's exact count. This is a display
range, not a context limit or pressure threshold.
Cache counts appear only when reported for the selected request. Aggregate cache
statistics and prefill rates are never substituted for request-specific data.

Comparisons are labeled **PREVIOUS OBSERVED**, and are suppressed across changes
of provider or model. They do not establish conversation membership or explain
latency on their own. Request identifiers and sampled output counts remain in
Journal instead of occupying an Overview table.

Up to 240 distinct requests are retained. Repeated polls update an existing
observation, and past requests stay available. Use **Shift-↑ / Shift-↓** or
**PgUp / PgDn** to browse, **Home** for latest, **End** for oldest, and **r** to reset history.

## Operator charts

**Memory** shows system resident occupancy in cyan, with a separate pressure
severity label. **GPU** shows utilization history, with hardware and allocation
details in SYSINFO. **Paging / I/O** shows byte rates, separate input/output
rates and a SWAP capacity bar. **Cache** always shows interval history with
cumulative reuse kept in text. **Queue** shows active and waiting requests.
All time-series panels share a visible sample window and zoom in Overview.

Chart labels such as `window 34s` describe the rolling history span, not elapsed
runtime. The span stays fixed while new samples enter on the right. Resize the
terminal, expand a chart or change zoom to change the visible span.

GPU and paging traces retain green/yellow/red value bands.
GPU defaults are green below 75%, yellow from 75% and red from 90%; these are
load bands, not proof of a bottleneck. Configured thresholds apply consistently
to numeric readings, traces and device meters. Percentage axes stay at 0–100;
all other axes use the real measured unit and fit the visible data.

**Queue** plots active requests in cyan and waiting requests in yellow on one
shared, labeled zero baseline. A white `═` marks equal measured counts. Overlapping connectors merge into continuous muted lines and junctions.
Muted `≈` marks unequal values that coincide at terminal resolution. Exact
counts remain visible in the panel. Both series share an automatic request-count scale based on their visible
maximum. Idle zeros are valid; stale, missing and client-reported values
produce gaps. Queue length is a demand signal, not a latency measurement.

**First token** appears only after an explicit client timing is supplied in the
existing usage JSONL envelope:

```json
"timings": {"time_to_first_token_ms": 1250, "output_tokens_per_second": 40.0}
```

`output_tokens_per_second` is an optional finite, nonnegative completed-request
output average. It does not change the live generation chart.

Use a nonnegative integer measured from request dispatch to the first generated
token. Add this field alongside `provider`, `request_id`, `observed_at` and
`usage` in a complete record. The chart uses one column per observed request at 1×,
shows missing timings as gaps, and automatically scales its millisecond axis
to the visible requests. Its latest measured value is labeled REPORTED with observation age.
Repeated polls update the same request rather than adding duplicate bars.
Prompt-evaluation duration, generation speed and polling intervals are never
used to estimate first-token latency. No provider changes are required.

When first-token data is available and the terminal is at least 120 columns
wide, its chart sits beside the recent Journal. The Journal keeps 70% of that
row, and remains available in full through its tab. Without measured timing,
the preview uses the full row and no empty latency panel consumes space.


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

### Provider endpoints

Automatic selection follows the detected provider. Explicit `MLXTOP_PROVIDER`
selection takes priority and also works when the server runs on another host.
The dashboard monitors one selected server at a time; use separate terminal
sessions to monitor independent endpoints concurrently.

| Provider name | Default port | Read-only endpoints |
| --- | --- | --- |
| `ollama` | 11434 | `/api/ps` |
| `lmstudio` or `llmster` | 1234 | `/api/v1/models`, fallback `/api/v0/models`, then `/v1/models` |
| `vllm` | 8000 | `/metrics` |
| `sglang` | 30000 | `/metrics` (requires `--enable-metrics`) |
| `llama.cpp` | 8080 | `/slots`, `/metrics` (requires `--metrics`) |
| `koboldcpp` | 5001 | `/api/extra/perf` |
| `mlx-lm`, `localai` | 8080 | `/v1/models` |
| `jan` | 6767 | `/v1/models`; for older desktop servers set port 1337 |
| `gpt4all` | 4891 | `/v1/models`; enable the local API server in GPT4All |

```sh
MLXTOP_PROVIDER=ollama mlxtop
MLXTOP_PROVIDER=llama.cpp MLXTOP_PROVIDER_PORT=8081 mlxtop
MLXTOP_PROVIDER=vllm MLXTOP_PROVIDER_URL=https://inference.example.net mlxtop
```

`MLXTOP_PROVIDER_PORT` overrides the default loopback port.
`MLXTOP_PROVIDER_URL` overrides host/port and can include a reverse-proxy path
prefix. Supply the server root (for example `https://host/llm`), **without** the
endpoint suffix such as `/v1` or `/metrics`. HTTP and HTTPS are supported;
HTTPS certificates are verified. Set `MLXTOP_PROVIDER_API_KEY` in the environment
when the server requires bearer authentication. Remote credentials also require
`MLXTOP_ALLOW_REMOTE_AUTH=1`, preserving the existing authentication opt-in.
Keys are never printed or stored
in usage reports. Redirects and environment proxies are disabled; credentials
are sent only to the configured server. Requests time out, response sizes are
bounded and failures back off. Cached readings keep their original age and
become stale after five seconds. Invalid URLs or rejected authentication leave
telemetry unavailable; there is no unauthenticated fallback to another server.

oMLX retains its existing endpoint and login configuration; these provider
variables apply to the other adapters. API details appear in **Top** and
`--once` reports. With a remote URL, OS memory, GPU and process readings still
belong to the machine running mlxtop. Remote rates are excluded from local
hardware slowdown correlation. Run mlxtop over SSH for matching remote OS data.

- **oMLX:** reads active request IDs and prompt counts from admin statistics.
  Some engines/phases do not expose counts; untokenized queued zeros are skipped
  in the request journal. Queue counts, rates and output cover every loaded
  model; concurrent request rates and output counts are summed, and stay
  unavailable if any request omits its value. With several requests in flight,
  no single prompt size is shown. When more than one model is busy, the model
  reads `2 models · <first>` and the per-model prefix hit rate is not shown.
  For distributed (cluster) models, requests use rank zero's request IDs
  instead of oMLX's shared `rank0` placeholder, and stale rank-zero metrics are
  ignored.
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
- **Ollama:** `/api/ps` supplies loaded models, summed resident VRAM and a single
  model's configured context capacity. Residency does not establish request
  activity. The native completion recorder preserves decode speed and cached
  prompt tokens; total request duration is not used as decode time or TTFT.
- **LM Studio:** native v1 supplies loaded instances and their context capacity;
  v0 supplies loaded model identity. The final fallback supplies an available-model
  catalogue. Download size is never reported as allocated RAM. Native completion
  stats supply output speed and explicitly reported first-token time through the
  usage recorder.
- **vLLM/SGLang:** Prometheus polling sums active/waiting requests and computes
  server-wide generation/prompt-token rates from successive counter samples.
  These are sampled server rates, not per-request averages. The first sample,
  counter resets, changed series and gaps longer than five seconds produce no
  rate. vLLM also supplies interval and cumulative prefix-token cache reuse;
  SGLang supplies prefix hit rate when a single series is unambiguous.
  Top shows maximum reported KV occupancy across engines and cumulative mean
  TTFT. Aggregate latency is never inserted into individual request history.
  No per-request prompt counts or IDs are inferred from aggregate counters.
- **MLX-LM, LocalAI, Jan and GPT4All:** `/v1/models` supplies the available model
  catalogue. This is not proof that models are loaded or processing a request.
  Completion usage needs client integration, and missing counts/rates stay
  unavailable. LocalAI aggregate usage is not treated as individual requests.

### Client-reported usage file

A client can append one counters-only JSON object per completed request to a
local JSONL file, then launch mlxtop with:

```sh
MLXTOP_USAGE_FILE=/absolute/path/usage.jsonl ./target/release/mlxtop
```

For all live native adapters, the file supplements polling: completed requests appear
in prompt history while live slots and queue counts remain available. Completed
prompt counts are never combined with an active slot's output to estimate context.
For other runtimes, valid file records take priority over native last-result
reports (and over oMLX polling). An empty or invalid file does not disable
native polling.

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
`localai`, `vllm`, `sglang`, `jan`, and `gpt4all`.

Copy only usage counters from the response, with the required envelope above:

| Response format | Counter fields |
| --- | --- |
| OpenAI-compatible usage, including MLX-LM | `usage.prompt_tokens`, `usage.completion_tokens`, optional `usage.prompt_tokens_details.cached_tokens` |
| Responses-style usage | `usage.input_tokens`, `usage.output_tokens`, optional `usage.input_tokens_details.cached_tokens` |
| Ollama native | `prompt_eval_count`, `eval_count`, optional `prompt_eval_cached_count`; decode speed from `eval_count / eval_duration` (nanoseconds) |
| LM Studio native | `stats.input_tokens`, `stats.total_output_tokens`, `stats.tokens_per_second`, `stats.time_to_first_token_seconds`; `model_instance_id` is accepted as the model |

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

It accepts the response formats in the table for every listed provider, normalizes
native timings into the allowlisted `timings` envelope and excludes response text. For SDK
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
and `ps`. It reads configured provider endpoints and local logs only for
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
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo build --release --locked
```

CI also enforces at least 90% production Rust line coverage on macOS and Linux.
Run `scripts/coverage.sh` after installing cargo-llvm-cov and the matching LLVM
tools. The [coverage instructions](../CONTRIBUTING.md#coverage-gate) explain
setup, the test-file exclusion, reports and the HTML view.

For UI changes, test both the interactive dashboard and `mlxtop --once` and
include a terminal screenshot with the pull request. See
[CONTRIBUTING.md](../CONTRIBUTING.md) for design boundaries and contribution
licensing. The longer-term provider and architecture plan is documented in
[OPEN_SOURCE_ROADMAP.md](https://github.com/maximpri/mlxtop/blob/main/OPEN_SOURCE_ROADMAP.md).

## Deployment helper

### Private RC testing over SSH

Use Python 3.8+ and an SSH alias or `user@host`. To build on the oMLX server
and download the result to this Mac, run this in the server's RC checkout:

```sh
python3 scripts/rc.py build --stage-for-fetch
```

Then on this Mac:

```sh
python3 scripts/rc.py fetch omlx-server
python3 scripts/rc.py run
```

You can also build this Mac's working tree and test it on the server:

```sh
# Build the current working tree and stage it separately on the oMLX server.
python3 scripts/rc.py push omlx-server

# Test the server's staged RC in an SSH terminal (q exits).
python3 scripts/rc.py run omlx-server
python3 scripts/rc.py run omlx-server --once

# Download a staged server RC to this Mac and test it locally.
python3 scripts/rc.py fetch omlx-server
python3 scripts/rc.py run
```

Replace `omlx-server` with your SSH host alias. `push`, `fetch` and remote `run`
accept `--port`, `--identity` and `--dry-run`. To test only on this Mac, run
`python3 scripts/rc.py build` followed by `python3 scripts/rc.py run`.

The build includes uncommitted changes and requires Rust plus the native build
tools. On macOS it selects the SDK belonging to the active Xcode toolchain.
The other host only needs Python 3 and a compatible OS/CPU architecture. A
Mac binary cannot run on Linux; build on a matching host before transferring.
Running locally measures this Mac; remote `run` measures the oMLX server.

Each artifact records the RC version, revision, dirty state, platform and a
unique build ID. Transfers verify SHA-256, reject incompatible platforms, and
check the executable's version before switching the separate RC symlink.
Local artifacts live in `target/private-rc/releases`; the local test command
uses `target/private-rc/current/mlxtop`. Remote RCs live in
`~/.local/share/mlxtop/rc/releases`, with `current/mlxtop` pointing to the tested
candidate. Failed verification leaves the previous RC active. Earlier builds
are retained. Neither stable `mlxtop` installations nor oMLX services are changed.
This workflow creates no Git tag, push, GitHub release or public download.

### Versioned deployment

The optional deployment script creates versioned remote releases and
updates a `current` symlink. A binary-only deployment sends only the locally
built executable:

```sh
cargo build --release --locked
BUILD_ON_REMOTE=0 ./scripts/cicd.sh --host example.local --user deploy
```

To deploy a release that GitHub-hosted runners built instead of building at
all, pass its tag. The script picks the archive for the remote's platform and
verifies it against the release's `SHA256SUMS` before installing:

```sh
./scripts/cicd.sh --release v2.1.1 --host example.local --user deploy
```

Useful overrides:

```text
REMOTE_DIR                 remote install root (default: $HOME/mlxtop)
SSH_KEY                    private key for SSH/SCP
BUILD_ON_REMOTE            1 to build on target, 0 for binary-only install
RELEASE_TAG                deploy this GitHub release (same as --release)
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

### Provider API references

The adapters follow the documented read-only APIs:
[Ollama running models](https://docs.ollama.com/api/ps),
[LM Studio model inventory](https://lmstudio.ai/docs/developer/rest/list),
[vLLM metrics](https://github.com/vllm-project/vllm/blob/main/vllm/v1/metrics/loggers.py),
[SGLang production metrics](https://docs.sglang.io/docs/references/production_metrics),
[Jan CLI](https://www.jan.ai/docs/desktop/cli), and
[GPT4All API server](https://docs.gpt4all.io/gpt4all_api_server/home.html).
