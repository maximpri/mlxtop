# Changelog

## Unreleased

### Journal

- **GPU rows mark real load changes.** GPU load wobbling around a line during
  processing, such as 84% → 79% → 81% or 99% → 94%, no longer writes a row
  each sample. The busy band turns on at `gpu_warn_load` and holds until load
  drops below `gpu_warn_exit`, and saturation clears 10 points below
  `gpu_critical_load`. A `GPU BUSY` assessment row now files under the GPU
  filter and replaces the duplicate "busy burst started" row.
- **"GPU busy" in Settings now sets GPU BUSY.** The assessment used a fixed
  80%; it now reads `gpu_warn_load` (default 75%), so the Settings value and
  the state agree.
- **Telemetry online is recorded once.** An idle server whose rate comes and
  goes between polls no longer logs "live API telemetry online" every few
  seconds. It logs again for a new provider or source, or after the server
  went away and came back.
- **Choose what the Journal records.** `journal.prompts`, `journal.llm`,
  `journal.queue`, `journal.memory`, `journal.paging`, `journal.gpu` and
  `journal.thermal` turn each kind of row off, from `config.json` or the new
  Journal section in Settings. System rows, assessment findings such as
  HEAVY PAGING or GPU THROTTLED, and a move to critical memory pressure are
  always recorded.

### Dashboards

- **Sushi servers report their own generation and prefill rates.** The
  `mlx-serve` adapter now reads the `sushi:` gauge names when a server
  publishes them instead of `mlx_serve:` (`sushi serve` does), beside the
  shared vLLM-shaped queues. Decode, prefill, per-request speeds and prefill
  progress work unchanged; `MLXTOP_PROVIDER=mlx-serve` stays the selection,
  and a running `sushi` process is detected as mlx-serve.
- **The prefill rate stays after the prefill finishes.** A finished
  request's prefill speed was held for three seconds and then replaced by a
  live zero, so a request that decoded for longer than that showed
  `PREFILL 0.0` for the rest of its run. The last measured speed now stays,
  labeled `LAST PREFILL`, until a newer prefill replaces it. A fresh
  dashboard seeds its first reading from the server's cumulative finished
  prefill counters instead of showing nothing. The chart does not redraw
  that held reading on every poll, and it is never labeled `LIVE`.
- **The cache panel keeps a reading through a long decode.** Prefix-cache
  counters only move when a request completes, so the interval rate went
  blank while a request decoded. When no completed request supplied an
  interval rate, mlxtop now uses the prefix reuse of the requests still
  running.

## 3.1.2 — 2026-10-22

Running requests now show their own generation speeds, llama.cpp reports a live
generation rate, and Linux detects AMD GPUs without extra tools.

### Dashboards

- **Each running request shows its own speed.** While the runtime reports a
  decode speed per request, the Overview queue lists its running requests in
  its own rows, each with the tokens it has generated and its speed, such as
  `8f31a2 1.7k · 41.0 tok/s`, so a client's own tokens/s can be checked
  against it while the generation panel reports the whole server. oMLX
  reports these for every generating request; mlx-serve now does too, from
  each session's `generated_tokens`, read through the same stepping window as
  the server's decode speed; Splash has one while a request runs alone. A
  request without one shows `—` and keeps its row. The panel keeps its size,
  and the history returns when nothing is running.
- **Prompt load shows the tokens each request generated.** On a panel tall
  enough to keep four rows of bars, a row under the size labels shows each
  request's generated tokens. Whether the row is there depends on the panel's
  height alone, so the bars never shrink partway through a session.
- **llama.cpp reports a live generation rate.** Active decoding slots supply
  the current output rate, while slots still prefilling do not count toward it.
  The adapter accepts both object- and list-shaped `/slots` responses.
- **Linux reads AMD GPU counters from amdgpu sysfs.** Overview shows utilization,
  VRAM and temperature without a ROCm install. A runtime-suspended card stays
  listed without polling counters that could wake it on older kernels.
- **mlx-serve session rates reset after a server restart.** Reused request IDs
  cannot inherit a previous process's per-request speed.

## 3.1.1 — 2026-10-20

A Settings tab edits the config in place, `Esc` asks before quitting, one
switch turns off alarms, a double-click enlarges a chart, and mlx-serve shows
prefill progress with a steadier decode rate.

### Dashboards

- **mlx-serve prefill shows how far it has got.** While a prefill runs, the
  prefill panel's footer reads `41.0k / 61.5k · 67%` from the server's
  `prefill_tokens_live` and `prefill_tokens_expected` gauges, and `--once`
  carries the same figures beside the prefill rate.
- **mlx-serve decode speed no longer stutters.** The server refreshes
  `generation_tokens_live` once every two seconds, so a one-second delta read 0
  or twice the true speed on alternating samples. mlxtop keeps the points where
  the gauge advanced and divides across a six-second span, holds the last real
  speed for four seconds after the gauge stops moving, and reads a live zero
  once nothing has been generated for that long.

### Controls

- **A Settings tab.** Press `4` to see every config option with its value and
  default, and edit it in place. A change is checked, applied without a
  restart and saved to `~/.config/mlxtop/config.json`; `x` resets an option to
  its default. Unknown keys are kept, and a file that doesn't parse is never
  overwritten.
- **Double-click enlarges a chart.** A double-click on an Overview chart
  enlarges it, and another restores it, like `Enter` and right-click.
- **`Esc` no longer quits straight away.** It still closes help, diagnostics,
  a filter or an enlarged chart; with nothing open it asks before quitting,
  because quitting discards the session's charts and Journal. `q` still quits
  at once.
- **`"alarms": false` turns off critical alarms.** One config switch silences
  the terminal bell and the banner; charts, the assessment and the Journal
  still show the condition.

## 3.1.0 — 2026-10-19

A redesigned Overview: every panel draws its data in the form that suits it,
text that said nothing new is gone, and icons, meters and motion carry state
instead. mlxtop also updates itself, follows oMLX serving through Splash, and
catches heavy paging on a Mac without swap.

### Dashboards

- **Memory history shows what RAM holds.** The memory chart was a line near
  92% almost all the time, because file cache fills RAM. Each column now
  stacks the sample's wired, app, compressed and cache memory at eighth-cell
  precision, with free memory empty at the top. A model load reads as wired
  memory jumping while cache is squeezed out. A ribbon along the bottom keeps
  each sample's pressure colour. Linux stacks app and cache, Windows memory in
  use; without composition counters the chart stays an area.
- **Chart forms by data.** Levels (GPU busy, memory without composition) are
  areas: a graded line over one neutral fill. Bursty per-second rates
  (generation, prefill, compression, paging) and the cache interval are bars
  from zero, one per sample as measured; a measured zero is a coloured
  baseline tick, unlike a missing sample. The queue's compact view marks
  requests (`● ● ○`), first-token bars have eighth-block tops, and the process
  table's MEM% has a meter. Full cells are drawn as background fills, so bars
  no longer stripe between rows.
- **Paging and compression against their warning rates.** Their axes never
  shrink below the warning rate, drawn as a dotted yellow line, so a trickle is
  a short bar instead of a panel-high slab. The swap row reads `swap off` when
  there is none, and the compressor row is its share of RAM.
- **Prefill progress.** A request still prefilling shows
  `PREFILL ▕████░░░▏ 52% · 34,078 / 65,536 · 4m 12s left` in prompt load, the
  time left from the live prefill rate, and the status badge reads
  `PREFILLING 52%`. It comes from oMLX's prefilling rows and Splash's
  `prompt_processed`.
- **Queued prompts are not live.** Requests waiting in oMLX's queue draw as
  hatched bars and read `QUEUED`, so a benchmark batch no longer shows six
  identical live bars while one runs.
- Rate panels too short for axes, and the row under compact gauges, draw a
  sparkline instead of "Enter: history".

### Less text, more signal

- The top bar has a heartbeat spinner that turns with every sample, the
  refresh interval, and the one window all Overview charts share, instead of
  `window` in seven chart titles.
- The runtime card replaces SYSINFO: model and engine, a status badge that
  spins while work is under way, the source's age only once it is 3 s old,
  a GPU memory meter graded at 90% and 97%, and thermal state only when it is
  not normal.
- The assessment leads with ✓, ▲ or ✖; evidence appears only when something
  is wrong, and a second line only for an action (→) or a missing reading
  worth knowing about.
- Prompt load without requests is one line, and the host charts get the rows.
  A live request drops its age and clock; the comparison reads
  `Δ +12,768 (+63.8%) vs previous` and appears only when there is one.
- Chart footers name their numbers (`tok/s`, `compressing / decompressing`,
  `page-in / page-out`, cache `since start / prefix`); "auto", "Includes file
  cache" and "older → now" are gone.
- Tab 2 is **Processes**, since it lists any runtime. Its totals sit in the
  table title. The Journal's header is one line.
- Up and Down in Overview move to the adjacent row by edge distance, so a
  short panel is no longer skipped for a taller one.

### Runtimes and host

- **oMLX through Splash.** oMLX 0.7.1 can serve a model through a Splash
  engine. mlxtop keeps oMLX as the provider even when the engine sorts first
  in the process list, names the engine (`oMLX · Splash`), and drops oMLX's
  own MLX memory readings, which no longer hold the model. oMLX's admin
  benchmark talks to Splash directly, so a busy GPU with nothing tracked reads
  **untracked** with "GPU busy outside oMLX's request tracking", not idle.
- **Splash on its own** feeds the standard dashboards: a request decoding
  alone gets its own output speed, finished requests become completed records
  for prompt load and the Journal, and the details line leads with problems
  (failures, Metal health, requests waiting for memory, thermal, pressure).
- **System paging on macOS.** The paging panel counted only swap traffic, so
  a Mac without swap always read `0 B/s` while it paged model weights from
  disk. It now counts file page-ins and page-outs too. File paging counts
  against the system only while memory is tight (warning pressure, or 10% free
  or less); page-ins at twice the critical rate are **Heavy paging**. A model
  loading on a roomy machine shows in green as "Disk reads · memory not
  tight".

### Install and update

- Add `mlxtop update`, which replaces the running binary with the latest
  release, and `mlxtop update --rc`, which also considers release candidates.
  It downloads the archive for this platform from GitHub, checks it against the
  release's `SHA256SUMS` and the binary's reported version, and renames it into
  place, so a failed update leaves the old binary. Homebrew installs are pointed
  to `brew upgrade mlxtop`; a root-owned install (the macOS package) needs sudo.
- Release candidates tagged `vX.Y.Z-rc.N` are built on GitHub's runners and
  published as prereleases, which installers and plain `mlxtop update` skip.
- The macOS installer mounts the disk image with `diskutil image` where it
  exists, so macOS 27 no longer prints `hdiutil` deprecation warnings.

## 3.0.0 — 2026-10-17

mlxtop now covers local LLMs beyond Apple Silicon: NVIDIA GPUs and Windows,
plus the mlx-serve and Splash runtimes.

- Windows (x86_64) support: memory, commit charge, processes and runtime
  detection through the Windows APIs, NVIDIA GPUs through `nvidia-smi`, logs in
  `%LOCALAPPDATA%\mlxtop`, a PowerShell installer (`scripts/install.ps1`) and a
  release zip. Windows exposes no paging counters through those APIs, so paging
  rates show as unavailable rather than zero.
- NVIDIA findings: the assessment names a model partly or fully on CPU
  (Ollama placement), a card with full VRAM (97%, held until below 94%), and
  thermal, power-brake or hardware clock slowdowns on a busy card. The GPU
  panel's STATE column shows the clock-limit reason, and full VRAM turns the
  card's memory reading yellow.
- Add Splash (`MLXTOP_PROVIDER=splash`, default port 8000), detected by its
  Metal engine process. It polls `/status` for the model, each running request's
  prompt and output counts with prefill progress, waiting requests, a generation
  rate from the engine's decode counter, the prefill speed of its prefill
  commands, prefix-cache reuse, Metal memory against its limit, DFlash draft
  acceptance, server TTFT p50, and released weights, thermal state and memory
  pressure. An API key goes in `MLXTOP_PROVIDER_API_KEY`.
- Add mlx-serve (`MLXTOP_PROVIDER=mlx-serve`, default port 11234) with process
  detection. It polls `/metrics` for queues, a live generation rate that counts
  tokens of requests still running, a prefill rate that follows the prompt being
  processed now, interval and cumulative cache reuse, and MLX active memory. It reads
  `/metrics.json` for the model and the running requests' prompt, cached and
  output counts; sessions that only hold a prefix cache are not listed. The
  server needs `--metrics`, and an API key goes in `MLXTOP_PROVIDER_API_KEY`.

## 2.1.1 — 2026-10-09

Stable release of the 2.1 candidates below (2.1.0-rc.1 through rc.5).

- Overview leads with host-critical resources: memory (pressure, RAM
  composition and the GPU wired limit), a new macOS compression chart and
  paging, then prompt load, then a compact throughput row, on one aligned
  three-column grid. Tall terminals give extra height to the host resources.
- The assessment leads with a verdict (`Healthy · no bottleneck`), uses
  sentence case, and a critical alarm replaces it without hiding SYSINFO.
- Every chart is graded green/yellow/red by its configured thresholds, and
  generation and prefill by their rolling baseline. Chart, assessment and
  Journal colors agree.
- Prompt times use the local zone, each reading appears once, cache readings
  are labeled `CACHED`, `interval` and `SERVER TOTAL`, and narrow terminals
  keep full view names and units.
- `d` Diagnostics and `mlxtop doctor` explain the assessment, connection and
  runtime setup.

## 2.1.0-rc.5 — 2026-10-08

- Grade every chart by its thresholds. GPU is red again from
  `gpu_critical_load`; paging is red from `swap_critical_rate`, and the
  "Paging active" finding turns red at the same rate. The memory trace keeps
  the pressure severity captured with each sample. Cache reuse is green from
  50%, yellow from 20% and red below, for the cache chart and the prompt's
  CACHED share.
- Grade throughput against a rolling baseline: generation and prefill turn
  yellow 10% (and 2 tok/s) below the median of recent live samples and red at
  30%; the assessment reports a 30% generation drop in red too.

## 2.1.0-rc.4 — 2026-10-08

- Lay Overview on one grid of three equal columns so panel edges align across
  rows. Prompt load spans two columns with Cache above Queue in the third;
  generation, prefill and GPU take one column each.
- Give tall terminals' extra height to memory, compression and paging. Prompt
  load stops at fourteen rows, throughput at ten and Journal at ten, instead of
  one prompt bar or a long Journal filling the screen.

## 2.1.0-rc.3 — 2026-10-08

- Reorder Overview around host-critical resources: memory, compression and
  paging lead with the remaining height; prompt load follows; generation,
  prefill and GPU become a compact fixed row (seven rows, four on short
  terminals).
- Add a macOS compression chart: compress + decompress traffic, stored versus
  compressor bytes with the ratio, a COMP/DECOMP split and compressor occupancy
  as a share of RAM. Decompression of model or KV pages is a leading slowdown
  signal that paging alone misses. Linux keeps memory and paging at half width.
- Show RAM composition (wired, app, compressed, cache, free) under the memory
  reading, marking the GPU wired limit from `iogpu.wired_limit_mb` or the
  runtime's Metal working set; the marker turns red when wired memory exceeds
  it.
- Make chart colors agree with the assessment at every threshold. Paging turns
  red only with a critical paging finding (thrashing, heavy paging, page-in
  recovery), not at 16 MiB/s of one-way traffic; compression keeps the
  finding's 64/32 MiB/s enter/exit hysteresis; Journal logs paging below the
  warning rate as green `light` paging; the compact paging label uses `Watch
  paging` from 1 MiB/s and `Paging active` from 4 MiB/s; the GPU wired-limit
  marker is yellow with a legend reason instead of an unexplained red.

## 2.1.0-rc.2 — 2026-10-07

- Lead the Overview assessment with a verdict: `Healthy · no bottleneck` in
  green when nothing is actionable and pressure is normal. GPU load is evidence,
  never the headline; findings use sentence case, and an idle runtime reads
  `Idle · waiting for the next request.`
- Show memory PRESSURE in the memory chart's reading slot, with bytes and
  resident percentage beneath it. Short memory panels use a capacity bar with
  exact bytes instead of a flat trace on a 0–100% axis.
- Cap throughput charts at fourteen rows and give taller terminals' extra rows
  to prompt load and Journal.
- Show prompt-load timestamps in the local zone used by Journal (UTC, labeled,
  only when the offset is unavailable).
- Show each reading once: SYSINFO keeps runtime prompt counters only when they
  differ from the prompt-load request, and prompt load omits a live speed equal
  to the generation headline. Label cache readings `CACHED` (request),
  `interval` and `SERVER TOTAL`.
- Never color GPU utilization red; `saturated` labels the yellow band, and the
  GPU scale reads `within target`, `busy`, `saturated`. Keep the queue's waiting
  series muted while nothing waits, and use cyan for the paging chart title.
- In Overview, show a critical alarm in place of the assessment rows so
  SYSINFO stays visible.
- Keep full view names from 80 columns, keep units on narrow rate charts, and
  replace `int`/`decode` abbreviations. Use one empty-state vocabulary, spaced
  paging units, whole-token medians, `3 full journal`, a swatch legend for
  Queue, threshold-based compact paging wording, unsplit partial prompt-bar
  cells, left-aligned Journal states and a separate `r` help entry.

## 2.1.0-rc.1 — 2026-10-04

- Add a compact Overview assessment and the read-only `d` Diagnostics panel,
  including evidence, provider capabilities, polling ages and setup guidance.
- Add `mlxtop doctor` for the same diagnostic report without a TTY, with explicit
  configuration and primary-connection failure exit statuses.
- Distinguish transport, authentication, malformed-response and optional-endpoint
  failures without exposing credentials or raw responses. Label local host
  findings when monitoring a remote runtime.
- Split the executable into domain, collection, provider, analysis, application,
  rendering and reporting modules; separate history storage from chart widgets.

## 2.0.0 — 2026-10-02

- Expand native monitoring to eleven local LLM runtimes. Add Ollama loaded
  models, resident VRAM and context capacity, and LM Studio loaded instances
  with native v1, v0 and OpenAI-compatible API fallback.
- Add vLLM and SGLang process detection and Prometheus polling: active/waiting
  requests, sampled server token rates, cache statistics, KV occupancy and
  cumulative mean first-token timing. Preserve gaps across resets, changed
  series and outages; never infer per-request history from aggregate counters.
- Add explicit Jan/GPT4All detection and model-catalogue polling, plus native
  catalogue polling for MLX-LM and LocalAI. Keep unavailable performance data
  unknown. Monitor one selected endpoint per session.
- Support configured HTTP/HTTPS provider URLs and bearer tokens with verified
  certificates, bounded reads and timeouts, and no redirects or environment
  proxies. Preserve the remote-authentication opt-in and exclude remote API
  rates from local hardware slowdown correlation.
- Preserve Ollama and LM Studio completion speeds, cached tokens and explicit
  first-token timings in the counters-only usage recorder. Extend usage reports
  to all eleven runtimes. Show native API details in Top and text reports.
- Synchronize Overview time-series windows and zoom across panel widths without
  dropping samples. Keep independent zoom for request bars and label rolling
  durations explicitly, for example `window 34s`.
- Remove the process-memory chart and give throughput and prompt bars more
  vertical space. Retain OS process readings in Top and the static report.
- Render resident RAM occupancy in cyan with a separate pressure-severity label.
  Keep cache interval history stable when data is missing; show cumulative reuse
  as text. Move server averages to expanded charts and label live rates.
- Draw overlapping queue connectors as continuous lines with proper junctions.
  Mark equality only for equal measured counts; distinguish crossings and
  unequal values rounded to the same row. Keep arrow navigation adjacent.
- Add a 90% production Rust line-coverage gate on macOS and Linux, with
  per-file reports and tests excluded from the denominator. Move tests into
  dedicated files and cover missing telemetry, provider failures, request
  speeds, navigation, rendering and journal events.

## 1.2.1-rc.8 — 2026-10-01

- Color RAM readings, gauges and history by the captured memory-pressure state.
  High resident usage including file cache no longer creates a false red warning
  while macOS reports normal pressure. Missing pressure remains unknown.
- Preserve historical pressure colors across percentage changes and chart
  connectors. Linux derives pressure from MemAvailable and PSI, honoring the
  configured memory bands; macOS uses its native pressure state.

## 1.2.1-rc.7 — 2026-10-01

- Show output count and request-specific tokens/second in prompt details and
  retained prompt history. Label active samples LIVE, completed averages AVG,
  and retained observations LAST, independently of server-wide throughput.
- Capture oMLX local/distributed decode rates and KoboldCpp completed rates.
  Accept explicit output speed in usage records and Ollama decode timing.
  Preserve the last measured speed and age when a later sample omits timing.
- Keep the prompt panel's existing size and bar labels; fit output speed into
  its metadata rows, with no separate chart or additional screen allocation.

## 1.2.1-rc.6 — 2026-10-01

- Correct RAM history to physical occupancy, including file cache, with resident
  bytes/total and an explicit resident label. macOS pressure accounting no
  longer masquerades as RAM usage. Keep OS pressure independently visible.
- Remove the artificial 1 B/s axis from measured idle paging windows; preserve
  the zero trace, missing-data gaps and any visible historical traffic.
- Add percent units to axes, identify OS process footprint, and distinguish
  last sampled rates, server averages and cumulative cache reuse.

## 1.2.1-rc.5 — 2026-10-01

- Describe GPU Journal events as saturated, eased or idle instead of critical
  faults. Use the measured utilization band for recovery colors and honor the
  configured saturation threshold. Missing samples never report idle recovery.

## 1.2.1-rc.4 — 2026-10-01

- Prioritize memory pressure, process footprint and paging in Overview's main
  row. Give them at least twice the height of supporting throughput charts.
- Cap the generation/prefill/GPU row at six rows and reduce prefill to 30% of
  its width. Preserve compact prompt history and the expanded recent Journal.
- Label system memory as load percentage and color the separate PRESSURE
  reading using OS pressure severity. Keep compact rate units readable.

## 1.2.1-rc.3 — 2026-10-01

- Rebalance Overview: generation and prefill sit side by side, prompt history
  stays within eight rows, and recent Journal gets up to ten readable event
  rows with message wrapping. Keep the Journal visible beside measured latency.
- Rebuild process memory with a prominent current value, PID attribution,
  byte-axis ticks, a fitted range, and peak/growth details that fit the panel.
- Move contextual keyboard controls to a bottom bar. Simplify chart headers,
  reserve detailed statistics for expanded views, and preserve complete Queue
  counts and paging labels at 80 columns.

## 1.2.1-rc.2 — 2026-10-01

- Anchor prompt history at the right edge with fixed request spacing, including
  a single observed request.
- Treat oMLX's initial prefill speed as unavailable until measured. Distinguish
  interval cache readings, last-sample age, window statistics and session averages.
- Make isolated chart samples visible, round traces to the nearest row and show
  exact Queue counts when a short panel cannot represent its numeric scale.
- Leave cache-history gaps across stale telemetry, counter resets, provider
  changes and inconsistent deltas instead of reporting false zero reuse.

## 1.2.1-rc.1 — 2026-09-30

- Size Overview by operational importance: large prompt history beside stacked
  generation/prefill, a substantial memory/paging row and smaller GPU/cache/queue
  panels. Keep empty and active geometry stable instead of collapsing prompt load.
- Replace the tall SYSINFO card with a dense full-width strip; preserve model,
  state, freshness, hardware and current work counters on short terminals.
- Give empty prompt history a deliberate waiting state. Preserve exact selected
  size, cache, age, UTC time and per-bar labels on narrow charts.
- Identify unallocated swap explicitly in the static report.
- Record importance-based sizing and stable geometry in the shared chart spec.

- Separate Cache and Queue into independent Overview panels. Consolidate cache
  statistics only with the Cache chart; preserve Queue's active/waiting counts,
  history, selection and zoom. Keep both counts visible on short terminals.
- Remove nested throughput/memory frames, cap SYSINFO width, and move hardware,
  thermal and GPU allocation details there. Preserve units on standalone charts.
- Add a horizontal swap-capacity bar alongside paging traffic. Use percent bars
  in compact panels, keep queue counts readable and label cache average fallbacks.
- Explain visible history gaps, label retained prompt models, fix directional
  chart selection, and support large queue axis values.
- Record the flat layout and metric rules in the shared chart specification.

- Show a compact prompt-size label beneath every visible prompt-load bar at
  every zoom level. Reserve space for readable token counts and keep the
  selected request's exact count and UTC timestamp in the headline.
- Replace throughput summary cards with generation and prefill charts; remove
  diagnosis cards and expand model/state with queue, request, process and
  telemetry details. Consolidate throughput, system/process memory, GPU,
  paging/I/O and cache/queue; give prompt history a full row.
- Fit numeric chart axes to visible data in their actual units, including
  paging bytes/s, token rates, prompt tokens, process bytes, queue counts and
  latency. Keep 0–100 only for percentages. Restore green/yellow/red GPU load
  bands and document the shared rules for all charts in `docs/CHART_SPEC.md`.
- Restore Tab/Shift-Tab as global view navigation; keep arrows for chart
  selection. Make MLX Top a process table with filtered totals, full commands
  and selected-process details; label provider-wide model/state separately.
- Build, transfer, download and run private RCs with `scripts/rc.py` over SSH,
  with checksum/platform verification and a separate RC installation.

## 1.2.0 — 2026-09-30

- Release the Linux multi-GPU dashboard, interactive chart controls, exact
  prompt-size readings and critical memory/paging alerts described below as
  stable. There are no functional changes from 1.2.0-rc.2.

## 1.2.0-rc.2 — 2026-09-30

- Advance the release candidate version and rebuild the distribution. There are
  no functional changes from the locally built 1.2.0-rc.1 candidate.

## 1.2.0-rc.1 — 2026-09-29

- Clarify that prompt load shows each request's exact input-token count,
  including cached tokens, and document the selected request's freshness.
- Select Overview charts with Tab, arrows or the mouse; zoom each history
  independently with +/− or the wheel, and enlarge/restore with Enter/Esc.
  Move sampling interval controls to { / }.
- Replace prompt-growth warning marks and advice with actual token counts,
  recent median and size range.
- Show measured slowdowns and concise evidence in Diagnostics; high GPU usage
  stays neutral and does not establish a compute bottleneck.
- Sound the terminal bell for critical memory pressure as well as severe paging,
  once per episode. Acknowledgment persists through unavailable samples.
- Report Linux temperatures without inferring throttling from a fixed threshold.
- On Linux, collect every NVIDIA GPU, with UUID-based identity, per-card utilization,
  VRAM and temperature. Keep unavailable readings distinct from zero.
- Add a responsive GPU device panel to Overview; use `[` / `]` to select
  cards and reveal additional rows on smaller terminals. GPU summaries and
  history use the explicitly labeled maximum utilization across cards.
- Include every NVIDIA device in `--once` reports and run Rust CI on Linux
  as well as macOS.

## 1.1.2 — 2026-09-29

- Publish static Linux binaries for x86_64 and aarch64; the terminal installer
  now supports Linux as well as macOS on Apple Silicon.
- Introduce release-candidate versions using `X.Y.Z-rc.N`, including support
  for RC version strings in macOS packages and disk-image names.
- Expand prompt history across the panel with one bar per request and a
  consolidated UTC timestamp readout for the selected observation.
- Keep llama-server live telemetry alongside client-reported prompt history;
  poll slots and metrics independently and sum output across all active slots.
- Read llama-server active/deferred queue gauges without treating average rates
  as live generation speed.
- Recognize runtime entrypoints consistently, including MLX-LM Python modules,
  KoboldCpp scripts, LocalAI, LM Studio's headless daemon and the Bionic app.
- Filter usage files by provider, deduplicate request IDs and accept
  Responses-style usage and LM Studio model instance identifiers.
- Add a tested counters-only Python client helper with concurrent append support.
- Distinguish KoboldCpp generation IDs after an observed uptime reset.
- Keep successive oMLX distributed requests separate in prompt history by using
  rank zero's request IDs instead of the shared `rank0` placeholder.
- Report oMLX queue counts, rates and output across every loaded model and
  concurrent request, rather than only the first active model and request.

## 1.1.1 — 2026-09-15

- Fix disconnected corners in process-memory and queue traces.
- Give active and waiting requests one labeled zero baseline. A white `═`
  marks overlapping trace cells; exact counts and overflow remain visible.
- Show explanatory messages instead of empty indicator plots when no samples
  exist.
- Shorten prompt insights in narrow panels and use fractional bar heights
  when request cache counts are unknown.
- Update the installer, downloads and documentation to v1.1.1.

## 1.1.0 — 2026-09-15

### Operator dashboard

- Integrate prompt load into the wide Overview grid beside generation and
  prefill, retaining a stacked layout for smaller terminals.
- Show request input counts, freshness, previous-request changes and recent
  comparisons. Display cached/uncached segments only when reported for that
  request, with bounded history and keyboard navigation.
- Add active/waiting queue history and OS process-footprint charts. Keep
  missing readings as gaps and identify the measured process by PID.
- Show macOS process footprint, lifetime peak and signed memory growth without
  modifying or restarting the serving runtime.
- Display first-token latency only when explicit client timing is supplied.
  Otherwise, retain the recent Journal preview in that space.
- Document consistent title casing, colors, layout and telemetry semantics.

### Providers and platforms

- Add request telemetry adapters for oMLX, llama.cpp and KoboldCpp, plus an
  optional counters-only usage file for client-reported responses, including
  MLX-LM, Ollama, LM Studio and LocalAI.
- Distinguish live observations from retained or reported results. Polling may
  miss requests that finish between samples; timing and cache data are never
  inferred from unrelated counters.
- Include Linux support using `/proc`, `/sys` and optional `nvidia-smi`.
  The published DMG remains for Apple Silicon macOS; Linux uses source builds.
- Add the macOS-only `libproc` dependency and its dependency license notices.

### Distribution

- Publish an Apple Silicon DMG with a native installer, documentation,
  dependency notices and SHA-256 checksum.
- Update the terminal installer and download links to v1.1.0.
- The binary targets macOS 11 or later and is tested on macOS 26.5.1. The
  installer is unsigned and the release is not Apple notarized.

## 1.0.0 — 2026-09-11

- Initial release with Overview, MLX Top, Journal, oMLX serving telemetry,
  macOS memory/paging/GPU inspection and the `--once` static report.
- Apple Silicon distribution with checksum verification and license notices.
