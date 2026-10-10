# Changelog

## Unreleased

Next major version (3.0), developed on the `next/3.0` branch.

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
