# Chart specification

This is the shared contract for **all charts**, including compact, expanded,
macOS and Linux layouts. New charts and redesigns must follow it. See
[UX design rules](UX_DESIGN.md) for the surrounding interface.

## Panels and consolidation

Overview uses flat sibling panels with **one border per chart**, never a
bordered group around already bordered charts. Size panels by their importance
to monitoring local inference and memory pressure, rather than giving every
metric an equal rectangle. The runtime card is a dense full-width strip, not a
tall card.

| Row | Space allocation |
| --- | --- |
| Context | Three-row runtime card, a two-row borderless assessment (one line, plus one for an action or missing reading), then the compact NVIDIA table when present |
| Grid | Three equal columns shared by every row, so panel edges align |
| Host | Memory, compression, paging one column each on macOS (memory spans two columns on Linux and Windows); all remaining height, at least six rows; process footprint stays in Processes and the static report |
| Requests | Prompt load spans two columns; Cache above Queue in the third; about 28% of the chart height, nine to sixteen rows (seven compact); until the first request the row scales with the terminal, five to twelve rows (six compact), when prompt load is one line |
| Throughput | Generation, prefill and GPU one column each; four to twelve rows, scaling with the terminal height |
| Recent events | Five rows including borders (three event lines), growing to fourteen on tall terminals; omitted on compact terminals |

Keep a stable layout across workload changes. At 170×42, the host row uses
fourteen rows, requests ten, throughput seven and Journal seven; at 250×80 the
host row grows to thirty-one while requests stop at sixteen. Compact
terminals retain seven request rows and exact readings when history cannot fit.
The compact assessment and NVIDIA device table consume the Journal preview budget; the full
Journal remains available with `3`. Measured latency may take 30% of a wide
Journal row. It must not replace Journal.

An empty state explains the absence of samples without fabricated bars or axes.
Request arrival, idle transitions, pressure and queue spikes must not rearrange
panels. Use the existing critical banner and semantic colors to emphasize pressure.
In Overview the banner replaces the assessment rows, keeping the runtime card's
model, state and data age visible during the incident.
NVIDIA identity and per-device readings retain a selectable table budget.

| Panel | Contents |
| --- | --- |
| runtime card | Provider, engine and model; status badge (spinner while working, prefill %); source age once 3 s old; on the bottom edge device, GPU cores, GPU memory meter (yellow from 90%, red from 97%), runtime memory and CPU; thermal only when not normal |
| generation / prefill | Separate tok/s bar histories with independent axes; no outer throughput box |
| prompt load | One labeled bar per observed request, cache split when reported, queued requests hatched; prefill progress for a live request still prefilling; generated tokens under the sizes when the panel keeps four bar rows |
| memory | PRESSURE in the reading slot; bytes and resident %; a composition bar of wired, app, compressed, cache and free RAM with the GPU wired limit (`iogpu.wired_limit_mb` or the runtime's Metal working set) marked, yellow with a legend reason when wired memory exceeds it; composition history over a pressure ribbon |
| compression | macOS compress + decompress traffic bars (B/s, axis at least `compression_warn_rate`), stored/compressor bytes and ratio, compressing/decompressing rates and the compressor's share of RAM; yellow from `compression_warn_rate`, never red |
| GPU | Utilization area and load state; hardware belongs in the runtime card |
| paging / I/O | Paging-rate bars (axis at least `swap_warn_rate`), page-in/page-out rates and a horizontal swap used/total capacity bar, `swap off` without swap |
| cache | Interval hit % bars with reuse since start and prefix hit kept in fixed text positions |
| queue | Independent active/waiting request history and counts; request marks when compact; on Overview, while the runtime reports per-request speeds, a list of running requests with their own speeds in the panel's own rows |

Cache and Queue must remain separate, independently selectable panels; never
merge them under a shared card. Consolidate cache readings only with Cache.
Do not add summary cards repeating these charts. Reserve space for recent Journal events; show explicitly measured first-token
latency alongside them when width permits. There is no
dedicated Diagnostics card. The borderless assessment shares the static report’s
diagnosis, with full evidence in the `d` overlay; existing alarms remain visible.

On short terminals, show percentage capacity bars and exact readings rather
than misleading one-row traces with 0/100 axes, with a sparkline of recent
samples in a spare row. Rate panels too short for axes draw a sparkline. Queue
retains both counts and its request marks even when history cannot fit. Enter
expands a selected compact panel into history.
Swap's capacity bar stays visible independently of paging-rate availability.
Cache never substitutes an aggregate gauge for its interval history. When
interval data is absent, show an empty state and retain the since-start and prefix hit text. A visible history window containing only
gaps must explain the missing samples, even if older off-screen data exists.

## Chart forms

**Choose the form from the data, not the panel.**

| Data | Form | Panels |
| --- | --- | --- |
| A level that persists between samples | Area: a graded line over one neutral fill (`AREA_FILL`); the line carries the grade | GPU busy; memory without composition counters |
| A composition that changes over time | Stacked columns at eighth-cell precision, softened kind colours, free empty at the top, the pressure grade as a `▀` ribbon on the bottom row | memory |
| A bursty per-sample rate or ratio | Bars from zero, one per sample, raw values; a measured zero is a coloured baseline tick, a missing sample an empty column | generation, prefill, compression, paging, cache interval |
| One value per request | Bars with eighth-block tops, ordinal spacing | prompt load, first token |
| A few discrete items | Marks: `●` running, `○` waiting, `+` overflow | compact queue |
| A share of capacity | Meter of horizontal eighth blocks on a dim track | swap, GPU memory, process MEM% |

Full cells are background fills, never `█` glyphs: many terminals draw block
glyphs with a gap between rows, which stripes bars. Fills are solid; do not
use `▓`/`░` textures to tell series apart, except the hatching that marks a
queued request. Saturated colours are for small marks and lines; large areas
use the neutral fill or softened colours, so no panel outshouts the rest.

## Units and automatic ranges

**Only percentage charts use a 0–100 axis. Every numeric chart automatically
fits its visible observations and labels the axis in the measured unit.**

| Measurement | Axis |
| --- | --- |
| Generation and prefill | tok/s; independent ranges with readable rounded ticks and headroom |
| Prompt size | tokens; zero-based automatic ceiling, size label on every visible bar |
| Paging / I/O, compression | B/s, KiB/s or MiB/s; automatic ceiling, never below the warning rate, never a normalized 0–100 score |
| Queue | requests; zero-based automatic ceiling shared by active and waiting |
| First-token latency | measured milliseconds; zero-based automatic ceiling |
| System memory, GPU utilization, cache percentage | 0–100%, explicitly identified as percentages |

Use the visible history after horizontal zoom/scroll to compute numeric ranges.
An old off-screen spike must not flatten the current trace. Generation around
35 tok/s should get an axis near that workload, not a hard-coded 100 tok/s
ceiling. A different provider/model starts a new throughput history. Handle
empty, zero, constant, very small and very large observations without division
by zero, clipping a valid visible peak or inventing observations.

Paging and compression floor their axis at the warning rate and draw it as a
dotted yellow reference line with its label, so traffic below the warning rate
stays visibly small and the threshold is always on screen.

Rescaling changes screen coordinates, never recorded values, sample order,
timestamps or captured colors. Numeric display ceilings are not context,
hardware or service limits. Changing horizontal zoom recalculates the numeric
vertical range; zooming does not change the sampling interval.

## Color contract

All charts use the same semantic palette. **Green, yellow and red encode
defined value bands or reported memory-pressure states; a series' identity
color must never replace its measured severity.** Match the current reading, trace and related gauge to the same rule.

| Band | Meaning |
| --- | --- |
| Green | Within the metric's normal/favorable band |
| Yellow | Elevated load or a measured condition to inspect |
| Red | Highest defined load band or a measured critical condition |
| Muted | Missing/unavailable data; never a healthy zero |

The current configured defaults are:

| Metric | Green | Yellow | Red |
| --- | --- | --- | --- |
| PRESSURE label and memory ribbon (or trace) | normal pressure | warning pressure | critical pressure |
| GPU utilization | below 75% | 75% to below 90% (`busy`) | 90% or more (`saturated`) |
| Paging rate (swap, plus file paging while memory is tight) | below 1 MiB/s | 1 MiB/s to below 16 MiB/s | 16 MiB/s or more, or a critical paging finding (swap thrashing, heavy paging, page-in recovery) |
| Compression traffic | below 64 MiB/s, or below 32 MiB/s once active | 64 MiB/s or more; stays yellow down to 32 MiB/s | none: no critical threshold is defined |
| Cache reuse (interval and prompt CACHED share) | 50% or more | 20% to below 50% | below 20% |
| Generation (dynamic) | at or above the rolling baseline | 10% and 2 tok/s or more below it | 30% or more below it, or a drop correlated with paging, memory pressure or thermals |
| Prefill (dynamic) | at or above the rolling baseline | 10% and 2 tok/s or more below it | 30% or more below it |
| GPU wired-limit marker | — | wired memory over the limit, with `wired over GPU limit` in the legend | — |

Dynamic bands compare each sample with the median of the chart's previous
thirty live samples (generation uses the assessment's slowdown baseline for the
same provider and model). Without three samples there is no baseline and the
sample is green. Prompt CACHED shares below 4,096 tokens stay neutral cyan.

Chart colors agree with the assessment where both grade the same signal:
"Paging active" turns red from the paging chart's critical rate, a generation
drop of 30% or more is red in both, and compression keeps the finding's
enter/exit hysteresis. GPU saturation is red on the chart while the assessment
still reports no bottleneck unless a slowdown is measured. Journal paging events
use the paging band: traffic below the warning rate is logged as `light`, in
green. The compact paging label uses the finding's words: `Light paging`,
`Watch paging` (from 1 MiB/s), `Paging active` (from 4 MiB/s).

RAM percentage and bytes still measure physical occupancy including file cache.
The memory chart's reading slot shows the PRESSURE label in its severity color;
resident percentage follows the bytes. The history stacks each sample's
composition, and its bottom-row ribbon keeps the pressure severity captured
with each sample, so a full but healthy RAM stays green. Without composition
counters the history is an occupancy area graded the same way. Below five
inner rows, memory shows bytes and a composition bar instead of a history.
Missing occupancy remains muted.

On macOS, paging counts file page-ins and page-outs as well as swap, because
a Mac without swap drops file-backed pages (model weights included) and reads
them back from disk. File paging counts against the system (chart grade,
Journal, assessment) only while memory is tight: warning or critical pressure,
or 10% free or less. Otherwise it is drawn green and labeled
`Disk reads · memory not tight`. Under tight memory, page-ins at twice
`swap_critical_rate` are heavy paging. A bar connector never takes a grade
above the samples it joins. Native macOS
pressure is authoritative. Linux derives pressure from unavailable memory
(`100 - MemAvailable%`) using configured 70/85 defaults, with full PSI stalls
of 1%/5% escalating to watch/critical. Occupancy is never a pressure threshold.

Use resolved configuration thresholds, not duplicated constants in renderers.
GPU bands mean utilization/load: `busy` is yellow and `saturated` red.
Utilization alone does not establish a bottleneck, so it never sounds an alarm
or becomes a finding without a measured slowdown. GPU Journal events use load terms
such as saturated, eased and idle; they must not call utilization critical or
report recovery from missing readings. Recovery colors follow measured load. Thresholds stay in the original
units when an axis changes. A history sample keeps its captured color when a
new sample arrives, the axis rescales, or the viewport changes.

The swap capacity bar uses cyan and labels used/total plus percentage. Occupied
swap alone does not imply paging traffic or an alarm; keep paging severity
attached to the separate B/s trace. Distinguish unavailable, no allocated swap
and measured zero usage.

Series without a defined health threshold must not invent one from their
display range: a fast token rate, large prompt or large footprint is not by
itself a fault. Use labeled identity/measurement colors for those series:
chart titles use their identity color (cyan; GPU blue). Throughput and cache
use the graded bands above. Queue identifies
active (cyan) and waiting series with stroke swatches, and marks exact equality
with white `═`. The waiting series is yellow only while the visible window
contains waiting requests; an empty wait queue stays muted. Overlapping
connectors use muted continuous strokes and proper junctions.
Unequal values rounded to one row use muted `≈`. Prompt bars distinguish reported cached tokens
(green) from live/retained uncached tokens (cyan/blue); size changes stay
numeric. A partial cell that would hold both segments stays unsplit in the
request color, so equal cache ratios never change color with rounding. Defining a new severity band requires recording its metric-specific
meaning and thresholds here, with boundary tests.

Keep numeric readings, units, state labels and legends readable without color.
Colors on chart titles or selected borders identify the chart/focus, not load.
Text uses text colors: a legend's swatch carries the series color, its words
stay in the text color. System memory labels its physical occupancy as
resident, with bytes/total. macOS uses total RAM minus free and speculative pages;
Linux uses MemTotal minus MemFree. Neither includes swapped-out bytes.
Missing counters yield a gap, never 0% or 100%. Resident occupancy does not
establish memory pressure. The PRESSURE label remains authoritative, including critical pressure at low
occupancy and normal pressure at high occupancy. GPU, paging and measured
throughput warnings retain their captured severity colors.

## Navigation

Tab/Shift-Tab always switch views; 1/2/3 open a view directly. Arrows and mouse
clicks select charts in Overview; Up and Down pick the panel in the adjacent
row that overlaps most, then the nearest edge, so a short panel is never
skipped for a taller one below it. Enter enlarges a chart; Esc restores it.
Keep these roles distinct. A one-row header holds the brand, views, a heartbeat
spinner that advances with every sample, the refresh interval and the shared
chart window; a bottom bar holds contextual controls and a persistent
help/quit hint.
Only add complete hint fields that fit. The footer names the focused chart and
its zoom, while expanded views expose detailed window statistics.

## History, labels and review

- Overview time-series panels share the same trailing sample window and zoom.
  Size the window to fit the narrowest plot, then widen the same samples across
  larger plots without dropping spikes or averaging observations. Missing data
  stays disconnected. Relative horizontal positions refer to the same sample.
  Label the common window once, in the top bar, e.g. `window 34s` (sampling
  cadence times sample slots), not in each chart title. This is a rolling span,
  not elapsed runtime; it remains fixed while the chart scrolls.
  Expanded charts use the available width for a longer history. Prompt/latency
  bars remain ordinal, with independent zoom; sampling cadence is unchanged.
- Selected prompt history identifies its provider/model when it differs from
  the runtime card, so retained requests cannot be mistaken for the current model.
- Prompt bars reserve enough width for every size label at every zoom level.
  Use fixed slots sized for the labels, right-aligning the newest request even
  when there is only one. Center each label beneath its bar; adding requests
  must not stretch sparse histories across the plot. Zoom widens the slots and
  reduces the visible request window. Horizontal spacing denotes request order,
  not time.
  `12.0k` means 12,000 tokens, not KB. The selected request retains its exact
  count, cache information and selection marker, plus its observation
  timestamp and age once it is no longer live.
  On narrow panels, prioritize the exact count, state and cache reading in the
  headline; output count and request speed use existing metadata rows. Keep the
  clock and comparisons when they fit; expansion reveals full metadata. The
  clock uses the host's local zone, like Journal, and is labeled UTC only when
  the local offset is unavailable. A live request speed equal to the generation
  headline is not repeated; a differing per-request speed is.
  Never sacrifice a reading or bar label to fit optional fields.
- Only measured live rates enter throughput traces. Session averages and
  retained results use `AVG`/`LAST` labels and never become live samples.
- oMLX prefill progress initializes `speed` to zero before a chunk rate exists;
  treat that placeholder as missing, retaining the separately labeled average.
- Distinguish Cache's interval reading and last interval age from aggregate
  reuse `since start` (and `prefix` hit rate), and both from the selected
  request's `CACHED` share in prompt load. An expanded chart's title labels its statistics `avg` and `peak` over the visible window; provider averages use
  `SERVER AVG` in expanded views; keep Overview focused on LIVE and LAST. Explain trailing gaps with the last sample
  and age, and mark isolated observations with a visible dot. Counter resets,
  stale intervals and inconsistent cache deltas leave gaps, never false zeros.
- Round area lines to the nearest terminal row; bars use eighth-cell tops.
  Short Queue panels without at least three plot rows show exact counts and
  request marks; they must not draw one request on a two-request tick.
- Preserve unknown counters as missing and keep provider/process boundaries
  disconnected. Scale and freshness must remain visible in expanded views.
- Review actual terminal output at 80×24, a medium size and a wide size,
  including live, idle, missing-data, zero, overflow and history-zoom cases.
- Test color boundaries, captured-color retention, real-unit axes, visible
  range scaling and the absence of duplicate panels when changing charts.

## Source semantics

- Apple [vm_stat source](https://github.com/apple-oss-distributions/system_cmds/blob/main/vm_stat/vm_stat.c)
  prints free pages excluding speculative pages, so both must be subtracted
  from total physical RAM to obtain the occupied/resident count.
- Apple's [memory_pressure source](https://github.com/apple-oss-distributions/system_cmds/blob/main/memory_pressure/memory_pressure.c)
  reads memorystatus level. XNU's [AVAILABLE_NON_COMPRESSED_MEMORY](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/vm/vm_page.h)
  includes active and inactive pages. Its inverse is not physical RAM usage.
- An all-zero paging window keeps measured zero samples and gaps, says
  No paging traffic in this window, and suppresses the artificial nonzero
  ceiling and middle tick. Real historical traffic retains its automatic axis.
- Percentage ticks include `%`. LAST SAMPLE identifies a polled observation;
  SERVER AVG identifies provider counters which may predate this monitor.
  Cache `since start` identifies aggregate reuse, distinct from interval samples.

- Prompt output speed belongs to the selected provider/model/request ID. Read
  oMLX generation speed or distributed per-request decode_tps, never prefill
  or aggregate throughput. LIVE marks active measurements; AVG requires a
  completed request with measured speed; LAST retains the last sampled rate
  and its age. Missing/invalid timing never becomes zero or another request's
  rate. A completion without timing must not promote a retained sample to AVG.
  Keep prompt panel allocation unchanged and preserve size labels on its bars.
