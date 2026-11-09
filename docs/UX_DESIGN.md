# mlxtop UX design rules

These rules apply to the terminal UI and to reviews of UI changes. Extend the
existing visual language before introducing a new convention.
The [shared chart specification](CHART_SPEC.md) governs all chart colors,
units, automatic ranges, consolidation and history behavior.

## Naming and typography

| Element | Convention | Examples |
| --- | --- | --- |
| Navigation | Title case | Overview, Processes, Journal |
| Operational section heading | Uppercase | RECENT JOURNAL, LLM PROCESSES |
| Chart title | Lowercase words; preserve acronyms | prompt load, generation, prefill, GPU |
| Status or compact metric label | Uppercase | LIVE, LAST SEEN, QUEUED, PREFILL, HISTORY |
| Explanation or suggested action | Sentence case | Inspect added context or large tool results. |
| Keyboard hint | Match the actual key and use a short action | p pause, Home latest |

Charts at the same level must use the same title casing, weight, spacing and
border treatment. A chart does not become an operational section merely because
it spans the full width. Preserve standard units and names: tok/s, GiB, GPU,
oMLX and MLX. Use bold for the primary reading or status, not every line.

## Say it once, show it if you can

- Every line of text must tell the operator something the screen does not
  already show. Delete labels that describe the interface ("Includes file
  cache", "older → now", "auto", a SCOPE line), restate a chart, or report a
  normal state ("THERMAL no warning", "0s old").
- Prefer a graphic to words: an icon for a verdict (✓ ▲ ✖, → for an action),
  a meter for a share of capacity, a mark per item for small counts (`●`/`○`),
  a progress bar with time left for work under way, and a spinner for
  activity. Motion means the sample clock advanced; a still spinner means
  sampling stopped.
- Name shared context once. The chart window and refresh interval live in the
  top bar, not in every chart title.
- Show notes only when they are actionable or explain a missing reading. An
  empty panel is one line, and its rows go to panels with data.
- Graphic elements still need a readable label or value beside them, and
  color alone never carries severity.

## Information hierarchy and space

- Overview answers: what is running, is it healthy, what is limiting it, and
  what should the operator inspect? Host-critical resources come first:
  memory (pressure and composition), compression and paging, then prompt size,
  which drives KV-cache memory. Throughput is the outcome and stays compact.
- Charts show trends and comparisons. Keep their primary reading, units and
  freshness adjacent. Keep supporting explanations shorter than the chart.
- Use one border per chart. Place a dense full-width runtime card above the
  histories; do not wrap charts in another throughput box.
  The runtime card owns the provider, engine, model and status badge, and on
  its bottom edge the hardware identity, cores, GPU memory meter and the
  runtime's memory and CPU; thermal state appears only when it is not normal.
  Put a fixed borderless assessment below it: an icon and the finding, with
  evidence only when something is wrong, then a second line only for the next
  check (→) or a missing reading worth knowing about. Lead with the verdict:
  when nothing is actionable and OS pressure is normal, say
  `✓ Healthy · no bottleneck` in green; high GPU use is evidence, never the
  headline. Findings use sentence case. Use `d` for full evidence and runtime
  setup; do not add another diagnosis card. A critical alarm replaces the
  assessment, never the runtime card.
  Show each fact once. The runtime card's title repeats runtime prompt counters
  only when they differ from the request shown in prompt load.
  Every row shares one grid of three equal columns, so panel edges line up.
  The host row takes all remaining height (at least six rows): memory,
  compression and paging on macOS; on Linux, which has no compressor counters,
  memory spans two columns beside paging. The memory reading slot shows
  PRESSURE; bytes and resident percentage follow, then a composition bar
  (wired, app, compressed, cache, free) with the GPU wired limit marked when
  known, and a history of that composition over a pressure ribbon.
  Compression charts compress + decompress traffic with the compressor's share
  of RAM; paging includes the swap capacity bar. Prompt history spans two
  columns of the next row (about 28% of the height, nine to fourteen rows; one
  line until the first request); Cache and Queue stack in the third. Generation,
  prefill and GPU take one column each in a seven- to ten-row row (four on
  short terminals, where readings stay in the titles). Journal grows from five
  to ten rows on tall terminals; everything else goes to the host row.
  OS footprint remains in Processes and the static report, without a chart.
  Show measured latency beside Journal when space permits.
- When the runtime reports each request, Queue lists them, one row each in
  fixed columns: a state mark (`◐` prefill, `●` generating, `○` queued), the
  uppercase state, the id's last six characters (Journal holds the full id),
  then the phase's own indicator: a progress bar and percentage while
  prefilling, tokens out and tok/s while generating (`—` without a rate), and
  prompt size while queued. Readings share the right edge. Work under way
  leads, so a short panel never hides the prefill holding the others up.
  Runtime counts sit in the top border (`●2 ○0` when narrow); `+N more` in
  the bottom border names rows that did not fit. Enlarged, the list sits
  above the history. The id goes first when width is short.
- Progress percentages round down: unfinished work never reads 100%, and
  less than a second left reads `<1s`, never `0s`.
- At very short chart heights, use percentage capacity bars and exact readings
  instead of one-row traces with misleading axes. Preserve units on standalone
  narrow rate charts. Keep an accessible help hint and separate chart/view keys.
- Give prompt history nine to thirteen rows on regular terminals (more on tall
  terminals, see above), seven on
  short macOS terminals and six with a compact NVIDIA table. Journal gets
  five rows on regular terminals (three event lines) and no compact preview; the full Journal is available with `3`. Wrap summaries to at most
  two rows, align time/state columns, and mark shortened messages.
- Keep a one-row header for views and sampling state (heartbeat, interval,
  shared window), and contextual controls in a bottom bar. Show current readings in chart headers, moving detailed
  window statistics into expanded views. Help and quit remain accessible.
- Empty and populated states have the same layout. Keep dimensions stable
  through live pressure, queue spikes and idle transitions; alarms and colors
  provide changing emphasis. The selected chart can always expand for detail.
- Keep exact selected values readable independently of bar height. Give prompt
  history its full allocated width, with a compact token-count label beneath every
  visible bar. Reserve enough width per request to keep these labels readable
  at every zoom level; anchor the newest bar at the right with fixed slot spacing,
  including a single request. Retain older requests through history navigation. Keep a
  single numeric size summary in the bottom border. Do not put long
  request IDs or raw payloads in Overview; use Journal for request detail.
- For NVIDIA devices, put a compact per-card comparison directly below the
  operational summary. Preserve utilization, VRAM used/total and temperature
  on narrow terminals; add load state and a VRAM bar when width permits. Use
  `[` / `]` to select cards and reveal overflow, with the visible range labeled.
  Keep card identities distinct and label the combined utilization `GPU max`.
- At narrower widths, remove secondary detail before clipping primary values.
  Keep status, throughput, request sizes and resource readings available. Avoid wrapping chart
  headers or allowing long identifiers to displace readings.

## Color and emphasis

Use the existing palette; do not introduce per-widget color schemes.

| Role | Treatment |
| --- | --- |
| Healthy or favorable measured condition | Green plus a label or value |
| Condition worth inspecting | Yellow plus an explicit reason |
| Critical measured condition | Red plus an explicit reason |
| Chart identity | Existing metric color; a title color alone is not severity |
| Live request in prompt history | Cyan plus LIVE |
| Historical request in prompt history | Blue plus LAST SEEN or REPORTED and age |
| Queued request in prompt history | Hatched bar plus QUEUED |
| Prompt size changes | Muted numeric comparison; never warning markers on bars |
| Resident RAM | Bytes and percentage; the PRESSURE label and the history's ribbon carry OS pressure severity |
| RAM composition | Wired blue, app cyan, compressed and cache two greys of different lightness, free empty; softened toward the panel in the history |
| GPU utilization | Green/yellow/red configured load bands with a numeric reading and load label (`within target`, `busy`, `saturated`) |
| Throughput | Green at or above the rolling baseline; yellow and red for measured drops of 10% and 30% |
| Cache reuse | Green from 50%, yellow from 20%, red below 20% (prompt CACHED share from 4,096 tokens) |
| Queue waiting series | Yellow only while requests wait in view; muted at zero |
| Supporting text, unknown data, historical advice | Muted |

Never require color alone to interpret severity, freshness or selection. Use
text, symbols and values alongside it. Text uses text colors; a series color
belongs on its swatch, line or bar, not on the words beside it. Large filled
areas use the neutral fill or softened colors, keeping saturated color for
lines, bars and marks. Preserve captured severity
colors in time-series history instead of recoloring old samples on refresh.

## Metric truth and operator advice

- Keep implementation scope within mlxtop. Use existing provider interfaces
  and operating-system metrics; do not patch, inject into, reconfigure or
  restart serving runtimes to obtain counters. Omit unavailable-only dashboard
  summaries rather than filling operator space with implementation limitations.
- Unknown is `—`, not zero. Distinguish idle, stale, live and reported data.
- Show observation age for retained request values. Redrawing must not refresh
  their timestamps. Label historical assessments HISTORY.
- State chart units and scales. Only percentage charts use 0–100; numeric
  axes automatically fit visible observations in their actual units. These
  display ranges are not model or hardware limits. Retain exact selected
  values. Keep time-series gaps disconnected and never drop captured samples to fit a plot.
- Compare like-for-like provider/model observations. Say previous observed
  request; do not assume conversation membership or a tool loop.
- Show cache reuse only when reported for that request. Do not substitute an
  aggregate cache metric or infer latency from token counts alone.
- The prompt-load headline is the selected request's exact input-token count,
  including cached tokens. Keep that total prominent when browsing history or
  zooming; cached and uncached segments partition the same total.
- Stacked prompt bars use green for reported cached tokens, and cyan/live or
  blue/historical for the remainder. Unknown cache reuse stays unsplit. Prompt
  changes appear numerically; do not mark growth as an alarm. `↑` marks scale
  overflow. Keep the selected `▲` beside its bar's size label. Compact labels
  use `k` for thousands of tokens and `M` for millions, never KB. The selected
  observation's local timestamp (Journal's zone; labeled UTC only when the
  offset is unknown) stays in the headline when it fits; output speed
  takes priority on narrow panels, with full metadata in the expanded view. Horizontal spacing
  represents request order.
  Cache availability must not alter bar height. Partial cells that cannot fit
  both segment colors and empty space stay unsplit in the request color; the
  selected readout retains the precise count and reported cache reuse (`CACHED`).
- Keep OS process memory in Processes and the static report, attributed to its
  PID and source. Keep footprint, RSS and allocator counters distinct.
- Suggestions must follow visible evidence. Prompt history emphasizes actual
  token counts and the recent median. GPU utilization alone establishes
  neither a slowdown nor a compute bottleneck. Diagnostics should pair a
  measured throughput drop with a supporting signal, label correlation and
  confidence, and give a short testable next step. Correlate events, not
  just states: find where the drop began and lead with the change just
  before it that still holds (a prefill starting beside generation, a request
  joining, a prompt jump, a pressure, paging, thermal or Metal-memory onset),
  saying how long before the drop it happened. Conditions that hold through
  normal work, such as a saturated GPU or a full Metal heap, rank below any
  such change and stay in the details; they are never offered as the reason.
  Measure decode speed as tokens produced between polls, not a runtime's
  average since the request began, which hides a stall. A generating request
  with no token for 1.5 s has stalled: say so first, with what the host was
  doing meanwhile, and journal its end with its length. Judge one request
  against its own earlier speed, smoothed over three polls, and only beyond
  twice its own jitter: speculative decoding swings a third poll to poll, and
  a request boundary is not a slowdown. When generation slows, its chart's
  bottom edge names the reason (`▼ …`) in the drop's colour, and Journal
  writes one STALL or SLOWDOWN row with the reason and the next step. Wrap advice instead of
  truncating it. Avoid generic tuning advice when no problem is established.
- Ring the terminal bell once when critical memory pressure or severe paging
  begins. Keep a visible, acknowledgeable banner. High GPU utilization alone
  must never trigger an alarm. Missing counters do not establish recovery.

## Interaction and review

- Preserve the three views: Overview, Processes and Journal. Place request load
  in Overview, with contextual history controls and a clear selected marker.
- Tab/Shift-Tab switch views globally, including from an expanded chart or
  an active process filter. In Overview, arrow keys select charts. Mouse clicks select,
  the wheel and +/− zoom the selected history, and Enter/Esc enlarge/restore.
  Time-series charts share their window and zoom in Overview, widening the
  same observations to fit different panel widths. Prompt and latency bars
  retain independent ordinal zoom. Sampling cadence remains unchanged.
  Show selection with border shape as well as color.
- Processes prioritizes the process table and selected-process details. Show
  filtered aggregate CPU/RSS and full commands. OS readings must belong to
  the selected PID; label provider-wide telemetry as runtime data rather than
  assigning a model/state to every process. Do not repeat Overview charts.
- Preserve 1/2/3 for views and p for pause. `d` opens read-only Diagnostics
  outside text entry; Esc/d closes it, and view switching dismisses it. Keep
  quit and alarm acknowledgment available and label retained paused observations. Shift-↑↓ and Home/End inspect prompt
  history. Use { / } for sampling cadence. Show contextual controls.
- Review at 80×24, a medium terminal, and a wide terminal. Check empty, live,
  idle/historical, unavailable and large-value states when affected.
- For layout changes, inspect an actual terminal rendering. For data semantics,
  use focused tests that verify freshness, comparisons and missing data. Follow
  the build and validation checks in [CONTRIBUTING.md](../CONTRIBUTING.md).
