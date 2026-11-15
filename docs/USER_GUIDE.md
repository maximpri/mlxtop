# mlxtop user guide

[Back to the README](../README.md) · [Quick start](../README.md#quick-start)

Detailed controls, runtime setup, and explanations of the dashboard readings.

## What it shows

| View | Question answered | Key information |
| --- | --- | --- |
| Overview | Is memory or paging limiting inference? | Memory pressure and composition, compression, paging, prompt history, throughput, GPU, cache, queue and recent events |
| Processes | Which process owns the workload? | PID, command, CPU, memory %, RSS, page-ins, OS state and selected runtime details |
| Journal | What changed during this session? | Request lifecycle, provider/model changes, paging, pressure, compression, GPU, thermal and recovery events |
| Settings | How is mlxtop configured? | Every `config.json` option with its current value and default, editable in place |

The **top bar** holds the views, a heartbeat spinner that turns with every
sample, the refresh interval and the time window every Overview chart shares,
for example `every 1s · window 46s`. When no sample has arrived for three
intervals it reads `NO SAMPLE 12s` in red instead of the spinner, and
`STOPPED` when sampling has ended (the assessment then says `Sampling
stopped`; the log has the reason).

Overview starts with the **runtime card**: the provider, engine and model, a
status badge, and the machine along its bottom edge (device, GPU cores, the
chip's peak memory bandwidth, a GPU memory meter graded at 90% and 97%, the
runtime's memory and CPU). The badge spins while work is under way and reads,
for example, `PREFILLING 52%` while one prompt is prefilled. The source shows
its age only once a reading is 3 s old, and thermal state appears only when it
is not normal. The card's top border repeats the runtime's prompt counters
only when they differ from the request shown in prompt load.

The bandwidth, for example `410 GB/s`, is the peak the chip's maker
publishes, and it bounds generation, which reads every weight for each token.
mlxtop looks it up from what it already reads: on a Mac the chip, its GPU core
count and its RAM, because bins of one chip differ (a 32-core M4 Max has
410 GB/s, a 40-core one 546, and an M6 153 GB/s with 16 GB but 170 with 24 or
32 GB); elsewhere the one card's name and memory. A chip it does not know, or
whose bin those readings cannot tell, shows no figure. It is the first field
to go when the edge runs short; `--once` prints it on the `METAL` line.

Below it, a one-line **assessment** leads with an icon: ✓ for
`Healthy · no bottleneck`, ▲ for a condition to inspect, ✖ for a critical one.
Evidence follows only when something is wrong, and a second line appears only
for a suggested check (→) or a missing reading worth knowing about, or, while
a healthy model decodes near the best this machine has reached with it, to say
that only the model's side can make it faster (see
[Decode efficiency](#decode-efficiency)).

The **host row** takes the remaining height: **memory**, **KV cache / model
I/O** and **paging / I/O** on macOS (memory spans two columns on Linux and
Windows). **Prompt load** spans two columns of the next row, with
**compression** above **queue** in the third. **Generation**, **prefill** and **GPU** share a compact
row, and the bottom **recent Journal** shows timestamped changes, newest
first. Select a chart and press Enter for its full view. Each panel keeps its
place when requests arrive or the model becomes idle; an empty prompt load is
one line, and the host charts get its rows.

**Memory** says what holds memory and how close it is to a limit, with the
OS **PRESSURE** label in the reading slot. It is laid out as the hardware is.

Where the GPU has no memory of its own (Apple Silicon, NVIDIA's integrated
GPUs such as DGX Spark and Jetson, which report no device memory, and AMD
APUs such as Strix Halo, whose model lives in GTT) there is one pool, and one
bar:

- **RAM** is memory in use (wired, app and compressed) against all of RAM,
  with the file cache as the bar's last segment. What the GPU holds is part
  of wired RAM and is the first part of the bar; where the runtime reports
  its model's size, that part splits into the model and `KV cache & other`.
  The rest of wired RAM follows as `other wired`. Near the GPU limit the
  GPU's part turns orange from 90% and red from 97%.
- Under the bar: how near GPU memory is to its limit (`GPU at 75% of 32.0
  GiB limit`) and what is left for a model to grow into. That is free memory
  plus the cache, which the OS gives back on demand, or the room under the
  GPU limit where that is less (`2.0 GiB to GPU limit`). On Linux the GPU's
  part is what an AMD APU holds (its BIOS carve-out and GTT), or what the
  processes on an integrated NVIDIA GPU hold (`nvidia-smi
  --query-compute-apps`); where that count is missing the line reads `GPU
  shares this RAM`.
- Under what is left, how much more context fits: the tokens each request
  in flight can still add, or with none in flight the next request
  (`≈37k tokens at 2 requests`). It is an estimate, in muted text, never a
  warning colour, and the first row to go: it is drawn only where every
  other row keeps its own. Diagnostics (`d`) gives the arithmetic.
- Then each part's size. A narrow panel wraps them onto a second row; it
  does not drop the sizes.
- **By app** names the runtime's own footprint and the largest other app.
  The header's `LLM` figure is the same footprint.
- Under them, the same pool over time in dimmed colours, against all of
  RAM: GPU memory, other wired RAM, app and compressed. A model load or a
  growing KV cache shows as the GPU band rising, another app as the app
  band. Samples marked as part of a slowdown are marked here too.

Where cards have memory of their own there are two pools, so **VRAM** and
**RAM** each get a meter with what is left, and a chart of their own.

Without composition counters the history is an area of occupancy.

Linux derives pressure from
unavailable memory (`MemTotal - MemAvailable`) and full PSI stalls;
`memory_warn_load` and `memory_critical_load` configure those Linux bands.
macOS uses native pressure rather than percentage thresholds. macOS
`memory_pressure -Q` includes pageable application pages in its reported
percentage and is not used as RAM usage.

**Compression** (macOS) shows the compressor's stored and compressed bytes and
ratio, compress plus decompress traffic as bars, and the compressor's share of
RAM. **Paging / I/O** shows disk paging as bars, page-in and page-out rates
and a swap capacity bar (`swap off` when none is allocated). On macOS paging
includes file page-ins and page-outs as well as swap: without swap, macOS
drops file-backed pages, model weights included, and reads them back from
disk. File paging counts against the system only while memory is tight
(warning pressure, or 10% free or less); otherwise it is drawn in green as
`Disk reads · memory not tight`, for example while a model loads. Both
charts' axes never shrink below their warning rate, which is drawn as a
dotted orange line, so a trickle stays a short bar. A measured all-zero
window says **No paging traffic** without inventing a ceiling; missing samples
remain gaps.

Only percentage charts use a 0–100 axis. Numeric charts automatically fit the
visible observations in their actual units: tok/s, tokens, bytes, bytes/s,
requests or milliseconds. Generation and prefill have independent ranges, so
a model running at 35 tok/s is not forced onto a 100 tok/s ceiling. Zooming the
history recalculates these ranges. The current axis is labeled; its limits are
display ranges, not hardware or model limits. See the
[chart specification](CHART_SPEC.md) for the rules shared by every chart.

Each chart's form follows its data. Levels (GPU busy, memory without
composition) are areas: a graded line over a neutral fill. Bursty
per-sample rates (generation, prefill, compression, paging) and the cache
interval are bars from zero, one per sample as measured; a measured zero is a
coloured tick on the baseline, while a missing sample leaves the column empty.
Rate panels too short for axes, and the row under compact gauges, draw a
sparkline of recent samples. Newest samples are at the right, and every
Overview time series uses the same trailing window. Wider plots widen those
same observations; no samples are dropped. Expanded charts show more history.

Rate charts contain only active-request telemetry. Server averages and
retained results stay explicitly labeled `AVG` or `LAST`; they are never
plotted as live samples. Missing, stale, idle and completion-log-only rate
samples leave gaps. A provider/model change starts a new throughput history.
When recent samples are missing, the chart labels the last visible reading and
its age. An expanded chart's `avg` and `peak` summarize the visible captured samples; `SERVER AVG` in
expanded views is the separate provider average, which may predate mlxtop.
Cache labels interval samples independently from its cumulative reuse since
start and prefix hit rate; it never switches to a cumulative gauge. oMLX's
initial prefill speed placeholder leaves a gap until a measured rate is
available. A short prefill can finish between polls without producing a live
rate sample; its server average remains available in the expanded chart.
Compact Queue panels show the counts with a mark per request (`●` running,
`○` waiting); Enter expands their history with a readable request scale.

Each sample keeps the severity colour recorded with it, so a later refresh
cannot recolor history. An area's line uses a causal deadband derived from
the chart's row resolution: movement of at most a quarter-row is held at the
prior plotted level, while larger changes pass through immediately, using only
earlier samples. Bars show raw values. Headlines and statistics always use raw
values. Colour is a secondary cue: labels use states such as `normal`,
`watch`, `critical`, `busy` and `saturated`, and verdicts carry an icon. On
narrower terminals, panels keep numeric readings when a secondary trace
cannot fit. Journal records transitions rather than duplicating the live
process table.

## Slowdowns

mlxtop finds slowdowns by anomaly detection, with one rule for every signal:
a sample is **unusual** when it is further from the signal's usual value than
the signal ordinarily strays. The usual value is the middle of the signal's
recent samples (about five minutes of generation, for one model on one
runtime). A runtime whose speed jumps about is rarely unusual; a steady one
shows a small change.

Generation speed depends on the work: a longer context and more requests
writing at once are slower. So a sample's speed is compared with earlier
samples of **similar work**: as many requests writing at once (1, 2, 3, or 4
and more), with a context (prompt and output so far) within a factor of two.
Work mlxtop has not seen before is learned for eight samples before it is
judged; it is never compared with different work.

Generation is **slow** when its speed is unusually low for its work (and at
least a fifth below usual) for three samples in a row. While no answer is
being written, **prefill** is judged the same way against earlier prefills of
similar prompts, and a slow one reads `PREFILL ↓61% (180.0→70.0 tok/s)`. A
sample in which a request started or ended is not judged: its tokens are
divided over an interval it worked for only part of. The reason given is
whatever else is unusual at the same time: thermal limiting, memory pressure,
paging, the runtime's disk work, a prompt prefilling beside the answer,
compression, the queue, model or Metal memory, the runtime's CPU, or a GPU
left idle. Request count and prompt size are never the reason: the usual
speed already allows for them. A signal at its usual level is never the
reason, however high that level is.

The generation chart's bottom edge says what speed it judges against, for
the work being done, for example `usual 30.0 for 2 requests · 131.2k
context`. During a slowdown a solid yellow line marks that usual speed on
the chart, so the gap under it is the drop; while all is well there is no
line.

Yellow means impact and nothing else. It appears only while the model is
slowed or stalled:

- the slowdown's samples on the generation chart;
- the samples on another chart whose reading was unusual beside it: paging,
  compression, memory, disk, or a GPU left idle;
- readings without a chart, in place: the header's `CPU 380% (usual 95%)` and
  `THERMAL` note.

A reading that is unusual while the model keeps its speed changes no colour;
Diagnostics lists it under `Unusual:`.

A reading that stays unusual for 30 seconds while generation keeps its usual
speed becomes the usual value. Requests, prompts and the queue are not marked
on their own; they are what you asked the runtime to do.

During a slowdown the same colour ties the panels together:

- the usual-speed line turns yellow and the reason is named under the bars;
- the border of the panel that shows the reason is tagged `▲ slowed GEN ↓50%`;
- the queue's active count and history are marked when the reason is how many
  requests waited.

An expanded generation chart (`Enter`) and Diagnostics (`d`) show what a
slowdown is judged against, for example `usual 50.0 · slow below 40.0`;
Diagnostics also lists everything unusual right now.

The Journal writes one row when a slowdown begins and one when it ends:

```text
SLOWDOWN   GEN ↓50.0% (50.0→25.0 tok/s) · paging 96.0 MiB/s (usual 2.0 MiB/s) → Reduce active models; recheck paging.
RECOVERED  generation back to 50.0 tok/s after 42s · low 25.0 tok/s
```

When nothing else is unusual the row says `nothing else unusual`, and if the
lower speed holds for 30 seconds it becomes the usual speed for that work
(`SETTLED`). This
is what happens together, not proof of cause. mlxtop needs eight samples of
generation before it has a usual speed to compare with.

## Decode efficiency

Writing an answer reads the model's weights for every forward pass, so the
chip's memory bandwidth bounds generation. mlxtop estimates how much of the
chip's published peak (the figure on the runtime card) decode reads, counted
per **verify pass**:

- **Passes per second** are the generation speed divided by the tokens a pass
  emits. With speculative decoding one pass of the model verifies a block of
  drafted tokens and emits those it accepts plus one of its own; the runtime
  reports that count. Without a draft a pass emits one token.
- **Bytes per pass** are what one pass reads, from the
  [model's facts](#model-facts): its weights, the KV cache of the context being
  decoded (KV bytes per token × the prompt and output so far) and the
  sequence's fixed state, counted once, as it is read.
- **The share** is passes per second × bytes per pass ÷ the chip's peak.

Counted per token instead, the figure is impossible: a Mac Studio (M4 Max,
32-core GPU, 410 GB/s) decoding Qwen3.8-27B at 123 tok/s with a draft that
lands 7.8 tokens a pass would read 450% of its peak. Per verify pass it reads
59% under 2k context, falling to 50% at 64k–128k, where attention work costs
time beyond the KV cache it reads. The draft's own reads are left out
(`draft excluded`): a runtime may hold the draft in another format than its
files (Splash loads a bf16 draft as 4-bit), so its bytes are not known.

It is an estimate, labelled as one and never coloured. It is shown only while
one request decodes alone on this machine and every input is known: nothing
for a remote runtime, a model partly in CPU memory, a chip whose bin mlxtop
cannot tell, a model whose bytes it does not know, an MoE model with a draft
(a pass that verifies several tokens reads the experts each of them routes
to), or a draft whose runtime does not report its tokens per pass. A share
above 100% means an input is wrong, and is not shown either.

No chip reaches its published peak, and how close it gets depends on the
chip, the quantization and the runtime (from about 40% to 95% in llama.cpp's
measurements), so mlxtop judges decode against the best it has reached on
this machine with this model in this session, while generation ran at its
usual speed: the share nine samples in ten of similar work (a context within
a factor of two) stayed under, once there are twenty. While the middle of the
last five samples is within a tenth of that best, a healthy assessment says
what would make decode faster:

```text
 ✓ Healthy · no bottleneck
   Decode is near its estimated limit: fewer bytes/token or a better draft.
```

Fewer bytes per token means a smaller quantization, a quantized KV cache, a
shorter context or a model with fewer active parameters; a better-matched
draft lands more tokens a pass. Without a draft, the line ends `or a draft
model`. Diagnostics (`d`) gives the share and the best, and with a draft what
the draft pays: its tokens per pass, and the speed this machine could not
exceed without it (one token a pass at the full peak):

```text
Decode estimate: 59% of 410 GB/s per verify pass, draft excluded; best 61%.
Draft: 7.8 tokens per pass; without it, at most an estimated 27 tok/s.
```

`--once` prints the same on `DECODE` and `DRAFT` lines beside the usual speed,
without a best, and the [session summary](#session-summary) gives decode by
context. The [JSON snapshot](#json-output) carries it as `efficiency`, and the
[Prometheus endpoint](#prometheus-endpoint) as `mlxtop_llm_decode_*`, with the
best from `mlxtop stream` and `mlxtop serve`, which watch the session.

## oMLX telemetry

The oMLX adapter checks `http://127.0.0.1:8000/health` by default, oMLX's own
default port. It discovers the host, port and optional API key from:

```text
~/.config/omlx-coding/server.env
```

Without that file, it takes the port from `OMLX_PORT`, then from `server.port`
(and `server.host`) in oMLX's `settings.json`, found in `OMLX_BASE_PATH` or by
default `~/.omlx`.

oMLX's admin statistics (speeds, requests, cache) need its API key. mlxtop
takes the first it finds: `MLXTOP_PROVIDER_API_KEY`, the **API key** in
Settings (`api_key` in `config.json`), oMLX's own
`OMLX_API_KEY`, an `API_KEY=` line in `server.env`, then `auth.api_key` in
oMLX's `settings.json`. It sends the key only to a loopback address (unless
`MLXTOP_ALLOW_REMOTE_AUTH=1`) and only when an oMLX process was found or oMLX
was chosen (`MLXTOP_PROVIDER=omlx`, or an `omx` host or port in the config);
another program answering on the port never receives it. With no key, the
Overview says `No live rates: oMLX admin key not set`, and the first time
that happens in a session mlxtop opens Settings on the API key, ready to
type (see [Settings](#settings)).

When the local admin API is available, values are marked `LIVE`. If it is not,
mlxtop may use the latest structured completion record from these logs and
labels that fallback with its age:

```text
~/.omlx-coding/logs/server.log
~/.omlx-coding/logs/launchd.stdout.log
~/.omlx-coding/logs/launchd.stderr.log
```

oMLX logs a request's tokens over the time after its first token, but over
the whole request, the wait for the first token included, when it recorded no
first token: an answer that was not streamed, which on one Mac Studio was most
short requests served through Splash. That figure is no generation speed, so
the fallback shows a logged rate only when it is clearly faster than the
request's tokens over its whole time, and leaves `GEN` unknown otherwise.

API credentials are sent only to loopback endpoints by default. To use an
intentionally remote oMLX endpoint, set `MLXTOP_ALLOW_REMOTE_AUTH=1` and use a
trusted, protected network path.

### oMLX serving through Splash

oMLX 0.7.1 can serve a model through [Splash](https://github.com/incoai/splash)
as an external engine: it starts `splash serve` on a private loopback port and
keeps reporting the model, requests and cache through its own admin API. mlxtop
stays on oMLX in that setup, even when the Splash engine sorts first in the
process list, and names the engine in the runtime card and report
(`oMLX · Splash · <model>`). oMLX's own MLX memory readings (active, cache,
peak, footprint) are dropped, because the model does not live in oMLX's MLX
runtime; device facts stay.

oMLX's admin benchmark sends its requests straight to Splash, so oMLX reports
nothing active while the GPU is saturated. When the GPU is busy and oMLX
tracks no active or waiting request, the status reads **untracked** rather than
idle, the assessment says `GPU busy outside oMLX's request tracking (oMLX
benchmark on Splash?) · no live rates`, and prompt load says it is busy outside
oMLX.

oMLX starts Splash on a loopback port it picks at each start, with an API key
it generates and hands to Splash privately, and passes on none of Splash's
draft counters; Splash answers `/status` only with that key. mlxtop therefore
cannot see tokens per verify pass, draft acceptance or what held Splash's work
up for a model oMLX serves through Splash, and Diagnostics says why:

```text
Draft: tokens per verify pass unknown · oMLX keeps Splash's status private
```

A Splash run on its own reports all of them (see [Splash](#provider-endpoints)).

### Model facts

For the one model oMLX serves on this machine, mlxtop works out what the
model costs per token and per sequence, and shows it in Diagnostics (`d`):

- **Read per pass**: the weight bytes one forward pass reads, from the JSON
  header at the start of each of the model's `model*.safetensors` shards. It
  counts the decoder layers and the output head, and leaves out what a text
  token never reads: a vision tower, an MTP head and the embedding table, which
  is only a lookup (unless the model ties it to the output head). For MoE models
  it counts the experts one token uses.
- **Bits per weight** over those bytes, scales and biases included, with the
  quantization from `config.json` (`4-bit g64` for 4-bit weights with a scale
  and bias per group of 64, `4/5-bit g64` where some layers differ, `mxfp4`),
  and the parameters.
- **KV per token**: keys and values for every layer that keeps them (a
  hybrid model's linear-attention layers do not), in the format the KV cache
  is stored in: for Splash, `integrations.splash_kv_format` in oMLX's settings
  (`int8` costs a byte a value and a byte of scale per 64 values); for oMLX's
  own engine, the model's activation dtype unless its `turboquant_kv_*`
  settings quantize the cache, whose size mlxtop does not work out.
- **State per sequence**: what a sequence holds whatever its length: the
  recurrent state of linear-attention (Gated DeltaNet) layers, full sliding
  windows, and with Splash the DFlash draft's context window it keeps beside
  each state.
- **Draft**: Splash always drafts with a DFlash2 checkpoint; its size is shown
  when the draft configured in `dflash_draft_model` fits the model. On oMLX's
  own engine, the model's MTP head, a DFlash draft or a vision model's
  assistant, as its settings say; a model without settings, or no
  `model_settings.json` at all, runs on oMLX's defaults, with no draft.
- **Context** oMLX allows for the model (`/v1/models/status`) and the context
  the model was trained for, and the oMLX and Splash versions.

A Splash run on its own states its model's KV layers, heads, format and cost per
token, the state each request holds, whether it drafts and the contexts in its
`/status` memory plan, and mlxtop shows those; it names neither the bytes a pass
reads nor the parameters.

The model's directory comes from oMLX's `/v1/models/status` and oMLX's version
from `/api/status`, asked once a minute and sent the same API key as the admin
login, under the same rules (the key alone, as a bearer token: no admin
session); the per-model settings come from `model_settings.json` in oMLX's base
directory. `/api/status` also names the custom kernels oMLX could not load,
which leaves the models that use them on much slower generic code, and the
models whose Neural Engine prefill was shed back to the GPU; for a model on
oMLX's own engine these lead its runtime details, in Diagnostics and the
Processes view's engine panel. mlxtop reads only
`config.json` and the shard headers, never the weights; it reads them once per
model and again only when a file changes, which it checks once a minute. A
file it cannot read leaves its facts unknown, and a remote oMLX's files are not
read at all. The JSON snapshot carries the same facts under
`runtime.model_facts`, and `mlxtop serve` writes them as metrics.

Other runtimes give what they report themselves, in the same lines: Ollama the
quantization, the context it is loaded with, and from `/api/show` the parameters,
trained context and experts (see [provider endpoints](#provider-endpoints)); LM
Studio the quantization, the context it is loaded with and the model's own; vLLM the
KV format and whether a draft is configured; SGLang the KV bytes per token, the
context it allows and whether it drafts; TensorFold whether it drafts.

For llama.cpp, the facts come from what llama-server says of itself, asked
every ten seconds through `/v1/models` and `/props`, which never wake a
sleeping server: the parameters and the size of the weights (so bits per
weight), the quantization (`Q4_K - Medium`), the context a slot allows and the
one the model was trained for, whether its slots draft speculatively, and the
build (`llama.cpp b8240-4f1e2a7`). llama.cpp reports the size of all the
weights, not what one pass reads (a mixture-of-experts model reads only a few
experts, and nothing in its API says whether a model is one), and no API gives
the KV cache's layout or format, so the bytes per pass and KV per token stay
unknown. The model's name is its alias, or the file name of the path it was
loaded from.

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

The thermal state on macOS is macOS's own thermal pressure level, read every
five seconds with `notifyutil -g com.apple.system.thermalpressurelevel`:
`nominal`, `moderate`, `heavy`, `trapping` or `sleeping`. From `moderate` up,
macOS is holding the Mac back for heat: the runtime card reads
`THERMAL heavy`, the assessment `Thermal limit`, and a slowdown at the same
time is put down to heat (`thermal pressure heavy`) unless the level was
already that high while speed was usual. `pmset -g therm` reports no CPU speed
limit on Apple Silicon; on Intel Macs mlxtop reads its `CPU_Speed_Limit` too
(`limited 70%`). A level that could not be read shows as `thermal —`, never as
nominal. macOS can keep the level at nominal while the GPU slows for heat, so a
nominal level does not rule heat out.

On macOS, Processes and the static report show OS process memory. Values come
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

The GPU dashboard is enabled on Linux when `nvidia-smi` detects at least one
NVIDIA GPU or the amdgpu driver exposes at least one AMD card, and on Windows
when `nvidia-smi` detects an NVIDIA GPU. macOS and systems without detected
cards use the standard Overview. Overview's **GPU
DEVICES** panel lists every detected card with its index, name,
utilization, VRAM used/total and temperature. Wider terminals add, in
order, a textual load state (from 104 columns), the card's power against its
cap (`183/450 W`, from 114), a VRAM bar (from 124), the graphics clock
against its maximum (`2520/3105 MHz`) and memory activity (`MEM BUSY`, the
share of time the card's memory was read or written; near 100% while
decoding means memory bandwidth is the limit) from 154, and the PCIe link
the card and its slot allow (`Gen4 x16`) from 184. A column appears only
when a card reports its reading, and a card without it shows `—`. Use `[` /
`]` to select a card; when the list is taller than the panel, the selected
card scrolls into view and the footer shows the visible range. At 80×24,
status, throughput, prompt sizes, GPU readings and prompt history remain
visible; historical charts return with more space. The **GPU** panel's
bottom edge adds what the cards draw together against their caps, and a
lone card's memory activity, while the edge has room.

Power, clocks, memory activity and the link come from an `nvidia-smi
--query-gpu` call of their own, apart from the core counters and the
clock-limit reasons (`power.draw`,
`enforced.power.limit`, `clocks.sm`, `clocks.max.sm`, `utilization.memory`
and the current and maximum `pcie.link.gen` and `pcie.link.width`): a field
a driver does not know fails the whole query, and the core readings must not
go with it. It runs every five seconds and its readings are kept between,
like a Mac's power reading; a query that fails outright is tried again a
minute later. `[N/A]`, `[Not Supported]` and anything else that is not a
number read as unknown, field by field. On Linux, `nvidia-smi
--query-compute-apps` lists the GPU memory each process holds, but only
where that answers a question: on an integrated GPU, and on a card at 94% of
its VRAM or more, where Diagnostics names who holds it.

AMD cards are read on Linux from the amdgpu driver's sysfs files under
`/sys/class/drm/card*/device`, with no ROCm install or extra tool:
`mem_info_vram_used` and `mem_info_vram_total`, `gpu_busy_percent`,
`mem_busy_percent` (memory activity), `mem_info_gtt_used` and
`mem_info_gtt_total` (system RAM the GPU maps), the top level of
`pp_dpm_sclk`, and from the card's hwmon sensors the `edge` temperature (the
junction sensor runs hotter; edge is the closest match to the core
temperature NVIDIA reports), `power1_average` (or `power1_input`) against
`power1_cap`, and the graphics clock `freq1_input`. Names come from the
driver's `product_name` when present, otherwise from the system's `pci.ids`
(`AMD Radeon RX 9070/9070 XT/9070 GRE`). AMD cards follow NVIDIA cards in the
list and are identified as `AMD-` plus the driver's `unique_id`, or the PCI
slot when the card has none. Clock-limit reasons are not read on AMD, so the
STATE column shows load only.

A card the kernel has put to sleep (`power/runtime_status` other than
`active`), such as a hybrid laptop's idle dGPU, stays listed with its VRAM
but without any other reading. On kernels before 6.13 the busy, sensor and
clock files wake the card when read, so polling them once a second would
keep it powered and drain the battery; and while it sleeps its VRAM is moved
to GTT, which would read as its own work. VRAM counters never wake it.

An APU (Strix Halo, and the Ryzen APUs before it) has no memory of its own:
its VRAM is a BIOS carve-out of system RAM, and a model on it lives mostly in
GTT. It is recognised by a carve-out under 1 GiB, or by the northbridge
voltage sensor (`in1_label` = `vddnb`) amdgpu gives APUs alone, so a large
carve-out is recognised too. Its memory reads as the RAM it holds, carve-out
and GTT together, against the most it may hold (`RAM USED / LIMIT`, `58.4/112.5
GiB`), and the memory panel draws one pool with the GPU's part inside RAM,
never a near-empty VRAM meter. An APU is listed only on a machine without a
card of its own: beside an NVIDIA or AMD card it drives the desktop, as
before. Its power reading covers the whole chip, CPU cores included, and
amdgpu offers APUs no memory activity and, on most, no power cap.

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
their UUIDs without pagination, with every reading each card gives. These
are physical-device readings; per-process memory is read only where it
answers a question (above), and MIG instances are not broken down. Renderer/tiler splits
and core counts are Apple-only and stay unavailable. Thermals come from
`/sys/class/thermal` (plus the NVIDIA or AMD card temperature when available).

With one card, the runtime card's bottom edge gives its peak memory bandwidth
when the card's name settles it. A name several cards share needs the card's
memory to tell them apart: amdgpu's `Radeon RX 9070/9070 XT/9070 GRE` is
640 GB/s with 16 GB and 432 GB/s with 12 GB, and an RTX 3080 has a figure
only as the 10 GB card. An RTX 4070 (sold with two kinds of memory), a bare
`A100` and Strix Halo, whose pci.ids name also covers a 128-bit part, show
none; neither do several cards.

## Controls

### Global

| Key | Action |
| --- | --- |
| `1` / `o` | Overview |
| `2` / `t` | Processes |
| `3` / `j` | Journal |
| `4` | Settings |
| `Tab` / `Shift-Tab` | Next / previous view, including while filtering processes |
| `p` / `Space` | Pause or resume sampling |
| `r` | Reset rates, charts and journal |
| `{` / `}` | Change refresh interval (1–60 seconds) |
| `a` | Acknowledge the current critical system alarm |
| `?` / `h` | Help |
| `q` / `Ctrl-C` | Quit |
| `Esc` | Close help, diagnostics, a filter or an enlarged chart; with nothing open, ask before quitting (`y`, `q` or `Enter` quits, any other key stays) |

### Diagnostics and runtime setup

Press `d` from any main view to open **Diagnostics**. During process-filter entry,
`d` remains a filter character; finish the filter first. The panel shows the full
assessment and what else was unusual when generation slowed. Local hardware findings are explicitly labeled when the selected
API is remote; they do not establish a remote inference bottleneck.
Beside the usual speed it gives the [decode efficiency](#decode-efficiency)
estimate, when one is known, and with a draft what the draft pays.

The connection section shows selection provenance, the resolved endpoint, hidden
credential status, polling ages, endpoint failures and capability sources. A
connected inventory API does not establish live throughput. Native live rates,
server averages, last completions, and client-reported usage retain their separate
meanings. A missing reading stays unavailable. Optional endpoint failures can
leave a connection partially available; legacy LM Studio fallbacks remain usable.

For a model oMLX serves on this machine, the connection section also gives the
[model's facts](#model-facts), each only when known. On a Mac Studio serving
Qwen3.8-27B through Splash:

```text
Model: 14.0 GiB read per pass · 4.69 bits per weight, 4/5-bit g64 · 26.9B parameters · draft 3.6 GiB
Context: KV 32.5 KiB per token, int8 · 152.6 MiB state per sequence · 131.1k tokens allowed of 262.1k trained
Version: oMLX 0.7.3 · Splash 1.5.0
Draft: tokens per verify pass unknown · oMLX keeps Splash's status private
```

Under the facts, how much more context fits, with its arithmetic. Tokens
left for each request = (room − margin) ÷ (KV bytes per token × requests in
flight), and no more than the context window less the most context one of
them holds. Requests in flight already hold their state, so it is not taken
off the room again; with none in flight, the next request's state is taken
off memory the machine has left, never off a runtime's plan, which keeps
state apart from its KV cache. The room
is the runtime's own where it reports it: the KV cache it has left in
tokens, or what its memory plan has left in bytes, taken as given. A runtime
that sizes its KV cache for its window when it loads a model (llama.cpp,
KoboldCpp, Jan, Ollama) has its window left whatever memory does. oMLX's own
engine, mlx-lm and mlx-serve grow the cache as tokens arrive, so their room
is what the memory panel says is left, less 1 GiB kept back for the system
and the runtime's working memory. Splash plans its memory itself and says
nothing until its plan is read. It needs the context window, the requests
in flight with the context they hold and, for a room in bytes, the KV bytes
a token costs; without any of them nothing is shown. On a Mac Studio whose
Splash plan leaves 5.02 GB for requests, two agentic contexts of 63.7k
tokens fill it:

```text
Context left: ≈1k more tokens for each of the 2 requests in flight (estimate)
By memory: 64.3 MiB left in Splash's memory plan ÷ (2 requests × 32.5 KiB a token) = 1.0k tokens each, the smaller
By window: 131.1k tokens allowed − 63.7k held = 67.4k tokens each
Held: 127.3k tokens in 2 requests: 3.9 GiB of KV cache and ≈610.5 MiB of state (305.2 MiB each), 98% of the KV cache Splash's plan has (4.0 GiB)
```

A Splash request in flight holds two state cells, so its state is counted
twice; the draft's attention cache beside them (40 MiB for Qwen3.8-27B's
DFlash2 draft) is not among the facts, so the state shown is a lower bound.

Once GPU memory reaches 90% of the GPU wired limit (`iogpu.wired_limit_mb`)
or of Metal's recommended working set, where MLX warns that generation can
be slow, the assessment here adds the advice in words: raise the limit
(`sudo sysctl iogpu.wired_limit_mb=…`) or shorten the context. It changes no
verdict and no colour; a slowdown blamed on model or Metal memory gives the
same advice as its next step.

For vLLM, SGLang and TensorFold it adds what their own counters say about
speculative decoding and the KV cache, each only when reported (see
[provider endpoints](#provider-endpoints)). On a vLLM server with a draft:

```text
Speculative decoding: 2.9 tokens per verify pass
KV cache: 41.2k tokens free · 4 requests preempted since the server started
```

For llama.cpp it adds what [llama-server](#provider-endpoints) says of its
slots, each line only while it says something: the context left for the
running request nearest its limit, a prompt being read, the slots and how
many shared a decode step, speculative decoding since the server started,
and whether the server is asleep:

```text
Context left: 1.8k tokens, for the running request nearest its limit
Reading a prompt: 12.3k tokens so far, 8.2k of them from the cache
Slots: 4 · 1.6 busy per decode step since the server started
Speculative decoding: 3.4 tokens per verify pass, 60% of drafted tokens accepted, since the server started
```

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

On compact terminals, the assessment uses the Overview Journal preview's
space. The complete Journal remains accessible with `3`.

### Overview charts

| Control | Action |
| --- | --- |
| Arrow keys | Select a neighboring chart |
| Mouse click | Select the chart under the pointer |
| `+` / `-` or mouse wheel | Zoom time series together, or selected request bars, from 1× to 8× |
| `0` | Reset time-series zoom, or selected request bars, to 1× |
| `Enter` / `Esc` | Enlarge / restore the selected chart |
| Double-click / right-click | Toggle enlarged view |
| `Shift-↑` / `Shift-↓` | Inspect newer / older prompt observations |
| `Home` / `End` | Select newest / oldest prompt |
| `PgUp` / `PgDn` | Move through prompt history by ten requests |
| `[` / `]` | Select a GPU card on Linux |

Time-series charts share zoom. Prompt and latency bars retain their own zoom. Zoom widens captured observations and shows a
shorter history range; it does not change the sampling interval or the values.
Expanded charts keep sampling, and Enter/Esc restores the dashboard. Hold the
terminal's selection modifier (usually Shift) to select text with the mouse.

### Critical system alarms

Critical memory pressure, heavy paging, swap thrashing and page-in recovery
ring the terminal bell once and display a banner. In Overview the banner takes
the place of the assessment, so the runtime card's model, state and data age
stay visible. Enable the audible bell in
your terminal settings to hear it. `a` dismisses the banner for the rest of
that episode; a confirmed recovery re-arms the alarm. Missing system counters
do not count as recovery. High GPU utilization is normal workload activity
and never triggers this alarm by itself. Set `"alarms": false` in the
[configuration](#configuration) to turn the bell and banner off.

Diagnostics distinguish measured throughput drops from resource usage. When
nothing needs action and memory pressure is normal, the assessment reads
**Healthy · no bottleneck**; GPU utilization appears as evidence beside it, and
a busy GPU alone never becomes the finding. A measured drop shows the rate
change, what else was unusual, and a short suggested check.
Linux temperature readings are shown as measurements, not inferred throttling.

### GPU findings

On Linux and Windows with NVIDIA cards, and on Linux with AMD cards, the
assessment names three GPU
problems that limit local inference. They rank below critical host memory,
swap thrashing, heavy paging and page-in recovery, and above memory stress
and host thermal limits:

| Finding | Measured from | Suggested check |
| --- | --- | --- |
| `Model partly on CPU` / `Model running on CPU` | Ollama `/api/ps`: `size` minus `size_vram` is at least 2% of the model | Smaller quantization or context; if nothing is on the GPU, the GPU driver |
| `GPU N VRAM full` | A card at 97% memory or more, held until it falls below 94% | Unload unused models or reduce context and KV cache |
| `GPU N thermal slowdown`, `power brake`, `hardware slowdown` | `nvidia-smi` clock-limit reasons on a card above 20% load | GPU cooling, power supply and cables |

Placement ranks first, then VRAM, then clocks. vLLM and SGLang reserve 90% of
VRAM by default, below the full threshold. A power cap under load is the card
running at its configured limit: the GPU panel's STATE column shows
`power cap`, but it is not a finding. Clock-limit reasons come from a separate
`nvidia-smi` query (`clocks_throttle_reasons.active`, then
`clocks_event_reasons.active` on newer drivers), so a driver without them
still shows utilization, VRAM and temperature. AMD cards report no
clock-limit reasons, so on AMD the findings are placement and VRAM only. On Windows a full card may
spill into shared system memory, which the driver does not report; the
finding says so. Offload is not reported for a remote runtime, whose
placement says nothing about local cards. An APU's memory is RAM, so it
never reads as `VRAM full`.

What else holds a card back is evidence, said in words in Diagnostics (`d`)
and under `GPU LIMIT` in `--once`, never a finding or a colour of its own:

- a busy card's clock-limit reason, with its power against the cap or its
  temperature, and its clock (`GPU 0 clocks limited by power cap · 448/450 W ·
  2010/3105 MHz`);
- a busy card's clock under two thirds of its maximum with no reason given;
- a busy card's PCIe link below what it and its slot allow (an idle card
  slows its link on purpose);
- who holds a card's memory, where that was read (`GPU 0 memory by process:
  ollama (77) 18.0 GiB · pid 912 4.0 GiB`);
- the system RAM (GTT) an AMD card with VRAM of its own holds beyond 1 GiB of
  staging buffers: a model spilling out of VRAM.

A thermal slowdown on an NVIDIA card is the thermal input the slowdown
detector judges, as a Mac's thermal state is: if generation slows while a
card is held for heat, and it was not while generation ran at its usual
speed, the slowdown's reason is `thermal limit` and the next step names that
card's cooling. While generation keeps its speed it is only listed under
`Unusual:`. A power cap is not that input: compute-heavy work such as a
prompt read beside the answer reaches it, and that work is the better
reason.

### Processes

The process table fills the view, with the process count and aggregate
CPU/RSS for the current filter in its title. On macOS a **FOOTPRINT** column
and total give what the OS charges each process for, GPU memory included: a
runtime holding a 23 GiB model can show 7 GiB of RSS, which leaves the
model out.
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

### Session summary

`u` opens what the session amounted to: how long the model was watched and
worked, the speed each cause cost, decode by context, where requests' time
went (queue, prompt, answer) with the slowest answers, the time the prompt cache saved, the
energy per token, and the peak memory of each model load. The same summary
is printed when mlxtop exits, if the model did any work. Energy is the whole
machine's power where macOS reports it; on Linux and Windows it is the power
every GPU reports (none if one card gives no reading), labelled `GPU power
only`, or `APU power only` for an AMD APU, whose reading covers its CPU cores
too.

A model load lasts while the runtime's live telemetry names the same model;
it ends when another model is named or the runtime goes away, and is kept
after that, the last eight loads in all. Its peak is the runtime's
footprint at its highest, the figure the memory panel gives as `by app` (on
macOS it includes the GPU memory the runtime holds), with the context in
flight at that moment, so a quant that fit only before its context grew
shows:

```text
Peak memory per model load:
  Qwen3.6-27B-4bit: 21.0 GiB with 64.0k tokens of context in 2 requests
  Qwen3.8-27B-oQ4e-mtp: 26.0 GiB with 1.9k tokens of context · now
```

Decode by context is the [decode efficiency](#decode-efficiency) estimate
over the session, for the model being served: for each band of context with
twenty samples of usual-speed decode, the usual share of the chip's peak, the
usual speed and, with a draft, the usual tokens per pass. The bands are under
4k tokens, then by factors of four:

```text
Decode per verify pass by context (estimate, draft excluded):
  under 4k  58% of 410 GB/s · usual 123 tok/s · 7.8 tokens per pass
  64k–256k  50% of 410 GB/s · usual 52 tok/s · 4.5 tokens per pass
```

### Journal

| Key | Action |
| --- | --- |
| `↑` / `PgUp` / `Home` | Move toward newer events |
| `↓` / `PgDn` / `End` | Move toward older events |
| `f` / `]` | Next event filter |
| `[` | Previous event filter |

### Settings

Settings lists every [configuration](#configuration) option, grouped into
General, Journal, oMLX server and Thresholds. A value set in `config.json` is shown in
white beside its default; an unset one shows the default in grey. Rates are
entered in MiB/s and stored in bytes per second.

| Key | Action |
| --- | --- |
| `↑` / `↓` / `Home` / `End` | Select a setting |
| `Enter` | Edit the value, or toggle an on/off setting (`Space` toggles too) |
| `Enter` while editing | Check and save; an invalid value is refused with the reason |
| `Esc` while editing | Cancel the edit |
| `x` / `Delete` | Reset to the default by removing the key |

**API key** is the key the monitored inference engine asks for: oMLX's admin
key, or the key a server was started with. It is typed and shown as dots,
never as its text; editing it again starts empty. Saving it makes
`config.json` readable by you only. `MLXTOP_PROVIDER_API_KEY` overrides it,
and the rules on where a key is sent are unchanged. The first time in a
session that the engine asks for a key mlxtop does not have, or refuses the
one in the file, mlxtop opens Settings on this field with the cursor in it:
type or paste the key and press Enter. Esc carries on without it; host
readings do not need a key.

A saved change takes effect at once and is written to `config.json`, so the
next start uses it too. Keys the view doesn't know are kept. A file that
doesn't parse is shown with the error and never overwritten; fix it by hand.
Changing the refresh interval or history here replaces a value given with
`--interval` or `--history` for the rest of the session. The file is read
again each time Settings opens, so edits made by hand show up.

## Command-line options

```text
-i, --interval N   refresh interval in seconds, 1–60 (default: 1)
-n, --history N    chart/journal history, 20–3600 (default: 300)
-1, --once         print a static report and exit
--json             print the static report as one line of JSON (implies --once)
--schema N         with --json or stream: stop unless the JSON is schema N
--record FILE      append every sample to FILE, one JSON line each
-V, --version      print the installed version and exit
-h, --help         show help

mlxtop stream [--interval N]   write the JSON snapshot of every sample, one line each
mlxtop serve [--listen ADDR]   answer /metrics, /snapshot and /health on loopback
mlxtop doctor [--interval N]   diagnose the connection and readings, then exit
mlxtop update [--rc]           install the latest release (--rc: or release candidate)
mlxtop replay FILE             read a recording back through the detection rules
```

`--once --json` and `mlxtop stream` are described under
[JSON output](#json-output), and `mlxtop serve` under
[Prometheus endpoint](#prometheus-endpoint).

A recording keeps every sample as mlxtop saw it: the machine's readings and
the runtime's, request by request, each with the moment it was taken (`ts`,
milliseconds since the epoch), so a recording that runs past midnight stays
in order. `mlxtop replay FILE` runs them through today's rules and Journal
and prints how many generation and prefill samples there were, how long
generation ran, how many samples fell inside a slowdown or stall, a tally of
the reasons given, and each finding with its time:

```text
GENERATION  7000 samples over 2h 0m · 21 in a slowdown or stall (0.3%)
PREFILL     0 samples · 0 in a slowdown (—)
REASON                      SLOW  EPISODES  PER HOUR
paging                        12         3      1.50
nothing else unusual           6         1      0.50
generation stalled             3         1      0.50
```

For each reason, `SLOW` counts the samples inside a slowdown or stall it was
given for, `EPISODES` the slowdowns and stalls the Journal began with it, and
`PER HOUR` those episodes per hour of generation; the reason told most often
comes first. That count is how a change to the rules is checked: record a
session in which the model slowed, replay it before and after the change, and
compare. A recording also goes with a bug report. A recording holds the
machine's readings, hardware names, the name of the process using the most
memory, and the runtime's request ids, model names and token counts. It never
holds prompts, output or process command lines, and it is created readable by
you only. Each sample adds about 3 KB, about 10 MB an hour at the default
one-second interval; nothing limits the file's size. Recordings made by 3.3.0 do contain the command lines of runtime
processes; check one before sharing it.

`mlxtop update` downloads the release archive for this platform from GitHub,
checks it against the release's `SHA256SUMS` and the new binary's reported
version, and renames it over the running binary, so a failed update leaves the
old one in place. `--rc` also considers release candidates, which are
published as GitHub prereleases. Homebrew installs are pointed to
`brew upgrade mlxtop`, and an install in a root-owned directory, such as the
macOS package in `/usr/local/bin`, needs `sudo mlxtop update`. Restart open
sessions afterwards.

## JSON output

`mlxtop --once --json` prints the static report as one line of JSON, and
`mlxtop stream` writes the same snapshot for every sample, one line each, for
scripts, agents and other tools:

```sh
mlxtop --once --json | jq .verdict
mlxtop stream | jq -c '{ts: .ts_ms, gen: .runtime.generation.tok_per_s, verdict: .verdict.state}'
mlxtop stream -i 5 >> samples.ndjson
```

Like `--once`, `--once --json` takes two samples an interval apart, so that
rates are measured. `mlxtop stream` takes a first sample to start the rates,
then writes a line every `--interval` seconds, or at the configured interval,
as the dashboard does, until it is stopped. It ends with exit status 0 and
says nothing when whatever reads it goes away (`mlxtop stream | head -3`) and
on Ctrl-C, SIGTERM or SIGHUP. A line is about 2.7–4.2 KB: about 3.2 KB for
a machine serving a model whose facts are known, about 3.8 KB while a slowdown
and its request are named, and about 4.2 KB when the runtime also counts its
KV room and draft, so decode efficiency and the context left are known too.
Standard output carries only JSON; a config file that was ignored is
said on standard error.

`mlxtop stream` and [`mlxtop serve`](#prometheus-endpoint) watch the session
as the dashboard does, so their snapshots carry what only a session knows:
the best decode reached here (`efficiency.best_bandwidth_share_ratio`), and
the note a healthy verdict gains near it (`verdict.next`). `--once --json`
watches no session, so it has neither.

The layout carries a `schema_version`, now 1, and is **experimental through
3.4**: until then a field may still change without a new number. After that,
fields are only added within a version, and renaming or removing one, or
changing its unit or meaning, takes a new number, so a reader should ignore
fields it does not know. `--schema N` stops mlxtop with an error that names
both numbers unless it writes schema N, so a script that pins its schema
hears about a change instead of misreading it.

Every field follows the same rules:

- Units are in the names: `_bytes` (a pass's, a token's or a sequence's where
  the name says so), `_bytes_per_s`, `_tok_per_s`, `_tokens`, `_s` (seconds),
  `_ms` (milliseconds; times are milliseconds since the Unix epoch), `_w`
  (watts) and `_celsius`. Shares are ratios from 0 to 1, named `_ratio`;
  nothing is a percentage. `relative_change` is a plain number: -0.5 is half
  as fast.
- A reading that is unknown is `null`, never 0: a counter that could not be
  read, a platform that has none, a runtime that does not report it.
- Where a figure comes from is the `source` of the object that holds it,
  which covers everything inside it: `host` (measured by mlxtop from the
  operating system), `runtime` (reported by the runtime, through its API, its
  log or a usage file) or `estimate` (mlxtop's own judgement from other
  readings: the usual speed and what is slow against it, which chip this is
  and so its published bandwidth, what the model costs, how much of that
  bandwidth decode reads, and how much more context fits). A figure whose
  source is not its object's names its own beside it: the GPU memory limit,
  which can come from either side, in `limit_source`; the published bandwidth
  in `published_bandwidth_source`; KV bytes per token, which the runtime may
  report, in `kv_bytes_per_token_source`.
- Values that name a state are lowercase words joined by underscores.
- There are no process names, command lines or process ids, so a stream can
  go to another machine. Runtime and model names stay.

### Fields

| Field | Meaning |
| --- | --- |
| `schema_version` | The layout's version, 1 |
| `mlxtop_version` | The mlxtop that wrote it |
| `ts_ms` | When the sample was taken |
| `interval_s` | Seconds between samples: what rates are measured over |
| `verdict.state` | The verdict by name: `healthy`, `no_bottleneck` (nothing found, but memory pressure is not reported normal), `collecting_baseline`, `counters_incomplete`, `generation_slowed`, `prefill_slowed`, `generation_stalled`, `requests_waiting`, `memory_bottleneck`, `memory_stress`, `swap_thrashing`, `heavy_paging`, `page_in_recovery`, `paging_active`, `watch_paging`, `compression_active`, `thermal_limit`, `cpu_offload`, `vram_full` or `gpu_throttled` |
| `verdict.severity` | Its icon on screen: `ok` ✓, `warning` ▲, `critical` ✖ or `neutral` ● |
| `verdict.title` | The headline, as the Overview shows it |
| `verdict.evidence` | The reading behind it |
| `verdict.context` | What else bears on it, as the static report's EVIDENCE line ends |
| `verdict.next` | The line under the headline, or `null` when there is none |
| `verdict.actionable` | `next` is a step to take (→), not a note |
| `host.source` | `host`: everything under `host` was measured from the operating system |
| `host.platform` | `macos`, `linux` or `windows` |
| `host.arch` | CPU architecture: `arm64`, `x86_64` |
| `host.chip` | The Apple chip, as its GPU is named (`Apple M4 Max`) |
| `host.gpu_cores` | The Apple GPU's cores |
| `host.ram_bytes` | All of RAM |
| `host.published_bandwidth_bytes_per_s` | The peak memory bandwidth the chip's maker publishes: the Mac's chip, or the one NVIDIA or AMD card, as the host footer shows it; `null` where mlxtop's table does not hold the chip or cannot tell its bin, and with several cards |
| `host.published_bandwidth_source` | `estimate`: which chip this is, and so its figure, is mlxtop's judgement from its name, GPU cores and memory |
| `host.memory.layout` | `unified` (the GPU's memory is RAM: Apple Silicon, DGX Spark), `dedicated` (cards with VRAM), or `null` with no GPU reading |
| `host.memory.pressure` | The operating system's memory pressure: `normal`, `warning` (shown as "watch") or `critical` |
| `host.memory.tight` | Pressure is raised or little memory is free, so file paging counts against the machine |
| `host.memory.available_ratio` | The share of RAM the operating system reports available |
| `host.memory.ram.used_bytes` | Wired, app and compressed memory: what the RAM meter fills |
| `host.memory.ram.left_bytes` | Free memory and file cache: what a model can still grow into |
| `host.memory.ram.wired_bytes` | Memory that cannot be paged out (on Linux, the kernel's and other memory that cannot be reclaimed) |
| `host.memory.ram.app_bytes` | Apps' memory |
| `host.memory.ram.compressed_bytes` | What the compressor holds (macOS) |
| `host.memory.ram.file_cache_bytes` | File cache |
| `host.memory.ram.free_bytes` | Free memory |
| `host.memory.gpu` | The GPU's part of RAM on unified memory, else `null` |
| `host.memory.gpu.used_bytes` | What the GPU holds (Metal's in-use count) |
| `host.memory.gpu.limit_bytes` | The GPU's wired-memory limit: `iogpu.wired_limit_mb`, else Metal's recommended working set as the runtime's MLX reports it |
| `host.memory.gpu.limit_source` | `host` for the macOS setting, `runtime` for the runtime's report |
| `host.memory.gpu.left_bytes` | The room left under the limit; 0 once over it |
| `host.memory.vram` | The cards' own memory, all cards together, where they have any; else `null` |
| `host.memory.vram.used_bytes` | VRAM in use |
| `host.memory.vram.total_bytes` | All VRAM |
| `host.memory.vram.left_bytes` | VRAM left |
| `host.memory.swap.total_bytes` | Swap allocated |
| `host.memory.swap.used_bytes` | Swap in use |
| `host.memory.swap.in_bytes_per_s` | Swapped in |
| `host.memory.swap.out_bytes_per_s` | Swapped out |
| `host.memory.swap.growth_bytes_per_s` | How fast swap use grows (negative while it shrinks) |
| `host.memory.file_paging.in_bytes_per_s` | Pages read from disk outside swap (macOS): model loads, and a full Mac's paging with swap off |
| `host.memory.file_paging.out_bytes_per_s` | Pages written to disk outside swap (macOS) |
| `host.memory.compression.compress_bytes_per_s` | Memory compressed (macOS) |
| `host.memory.compression.decompress_bytes_per_s` | Memory decompressed (macOS) |
| `host.gpus` | The Apple GPU of a Mac, or each NVIDIA and AMD card |
| `host.gpus[].index` | Its number |
| `host.gpus[].vendor` | `apple`, `nvidia` or `amd` |
| `host.gpus[].name` | Its name |
| `host.gpus[].busy_ratio` | How busy it is |
| `host.gpus[].renderer_ratio` | The Apple GPU's renderer, busy for this share |
| `host.gpus[].tiler_ratio` | The Apple GPU's tiler, busy for this share |
| `host.gpus[].memory_used_bytes` | A card's own memory in use |
| `host.gpus[].memory_total_bytes` | A card's own memory |
| `host.gpus[].temperature_celsius` | A card's temperature |
| `host.gpus[].throttle` | Why the driver holds a card's clocks down: `thermal`, `power_brake`, `hardware_slowdown`, `power_cap` or `none` |
| `host.thermal.level` | macOS's thermal pressure level: `nominal`, `moderate`, `heavy`, `trapping` or `sleeping` |
| `host.thermal.limited` | macOS is holding the machine back for heat: pressure from `heavy` up, or a CPU speed limit |
| `host.thermal.cpu_speed_limit_ratio` | The share of full CPU speed macOS allows (Intel Macs) |
| `host.thermal.hottest_celsius` | The hottest temperature measured: thermal zones and GPUs on Linux, GPUs on Windows |
| `host.system_power_w` | What the whole machine draws, where macOS reports it |
| `host.runtime_process.count` | The runtime's processes |
| `host.runtime_process.memory_bytes` | What the runtime holds: its largest process's footprint (GPU memory included) or the resident size of all its processes, whichever is more |
| `host.runtime_process.peak_bytes` | Its largest process's lifetime peak footprint (macOS) |
| `host.runtime_process.growth_bytes_per_s` | How fast that footprint grows |
| `host.runtime_process.cpu_cores` | CPU its processes use, in cores |
| `host.runtime_process.disk_read_bytes_per_s` | Its storage reads: a prompt cache on disk, or weights streamed from it (macOS) |
| `host.runtime_process.disk_write_bytes_per_s` | Its storage writes (macOS) |
| `runtime.source` | `runtime`: everything under `runtime` is what the runtime reported |
| `runtime.provider` | The runtime: `oMLX`, `Ollama`, `llama.cpp` … |
| `runtime.engine` | The engine it serves through when that is not its own (oMLX's Splash) |
| `runtime.model` | The model it names |
| `runtime.status` | Its state in its own word: `generating`, `prefilling`, `idle` …; `null` when no runtime was found |
| `runtime.telemetry` | Where its readings came from: `live_api`, `completion_log` (the last finished request in its log) or `reported_usage` (a usage file) |
| `runtime.fresh` | Its readings are live and current |
| `runtime.observed_at_ms` | When it last answered; its age is `ts_ms` minus this |
| `runtime.connected` | Its API answered the last poll; `null` when no runtime was found to ask |
| `runtime.remote` | It runs on another machine: the `host` readings are not its own |
| `runtime.details` | Its own facts in words, as the static report's PROVIDER line |
| `runtime.model_memory_bytes` | Memory its models take |
| `runtime.model_memory_ceiling_bytes` | The most memory it lets them take |
| `runtime.model_size_bytes` | The loaded model's size (Ollama) |
| `runtime.model_offloaded_bytes` | The part of the model it placed outside GPU memory (Ollama) |
| `runtime.context_allowed_tokens` | The context it allows for the model |
| `runtime.kv_free_tokens` | KV-cache room it reports left, in tokens: a pool its requests share (vLLM, SGLang, Splash), or for llama.cpp the room of the running slot nearest its limit |
| `runtime.kv_free_bytes` | KV-cache room it reports left, in bytes: what Splash's memory plan has left, or SGLang's free tokens at the bytes a token costs |
| `runtime.preemptions_total` | Requests it preempted (vLLM, TensorFold), retracted (SGLang) or suspended (Splash) for want of KV cache, its own count since it started |
| `runtime.model_facts` | What the model costs a pass, a token and a sequence, as mlxtop works it out from the model's own files (see [Model facts](#model-facts)); `null` when none of it is known |
| `runtime.model_facts.source` | `estimate` |
| `runtime.model_facts.weight_bytes_per_pass` | Weight bytes one forward pass reads for a token: the decoder layers and the output head; for MoE, the experts one token uses |
| `runtime.model_facts.bits_per_weight` | Bits per parameter a pass reads, scales and biases included |
| `runtime.model_facts.kv_bytes_per_token` | KV-cache bytes one more token of context costs |
| `runtime.model_facts.kv_bytes_per_token_source` | `runtime` where the runtime reports it, `estimate` where mlxtop works it out from the model's layers |
| `runtime.model_facts.state_bytes_per_sequence` | What a sequence holds whatever its length: linear-attention state, full sliding windows, a draft's context window; 0 for a model with none |
| `runtime.model_facts.context_trained_tokens` | The context the model was trained for, as its `config.json` (else the runtime) declares it |
| `runtime.model_facts.draft_weight_bytes` | The draft's weight bytes as stored, where speculative decoding drafts |
| `runtime.mlx` | The MLX allocator inside it, where it reports one; else `null` |
| `runtime.mlx.version` | Its MLX version |
| `runtime.mlx.active_bytes` | MLX's active memory |
| `runtime.mlx.cache_bytes` | MLX's cache |
| `runtime.mlx.peak_bytes` | MLX's peak |
| `runtime.mlx.process_footprint_bytes` | The serving process's footprint, as the runtime reports it |
| `runtime.generation.tok_per_s` | How fast the answer is written |
| `runtime.generation.basis` | What that rate is: `live` (over the last interval), `average` (the runtime's own, while it reports no live rate) or `last_request` (the last finished request's), as the screen's GEN, AVG GEN and LAST GEN |
| `runtime.prefill.tok_per_s` | How fast prompts are read |
| `runtime.prefill.basis` | What that rate is, as for generation |
| `runtime.prefill.done_tokens` | Prompt tokens read so far by the prefill in flight |
| `runtime.prefill.expected_tokens` | Prompt tokens it will read |
| `runtime.requests.active` | Requests running |
| `runtime.requests.waiting` | Requests queued |
| `runtime.requests.prefilling` | Running requests reading their prompt, where the runtime lists its requests |
| `runtime.requests.generating` | Running requests writing their answer, where it lists them |
| `runtime.requests.context_tokens` | Tokens the requests writing an answer hold, prompt and output together: the context their speed is judged at |
| `runtime.requests.prompt_tokens` | The runtime's prompt token count, as the static report's TOKENS line |
| `runtime.requests.output_tokens` | Its output token count, as the TOKENS line |
| `runtime.requests.list` | Each request it lists |
| `runtime.requests.list[].id` | The id the runtime gave it |
| `runtime.requests.list[].state` | `queued`, `prefilling`, `generating` or `completed` |
| `runtime.requests.list[].prompt_tokens` | Its prompt, cached tokens included |
| `runtime.requests.list[].cached_tokens` | Prompt tokens served from cache |
| `runtime.requests.list[].prefilled_tokens` | Prompt tokens read so far, while the prompt is read |
| `runtime.requests.list[].output_tokens` | Tokens written |
| `runtime.requests.list[].tok_per_s` | Its output speed: the latest interval's while it writes, its average once completed |
| `runtime.requests.list[].first_token_ms` | Time to its first token |
| `runtime.requests.list[].idle_s` | How long it has gone without a token, where the runtime says |
| `runtime.cache.reuse_ratio` | The share of prompt tokens served from cache since the runtime started |
| `runtime.cache.recent_reuse_ratio` | The same over the last interval |
| `runtime.cache.prefix_hit_ratio` | Prefix-cache hits |
| `runtime.cache.prompt_tokens_total` | Prompt tokens it was sent, its own total |
| `runtime.cache.cached_tokens_total` | Of those, the ones served from cache |
| `runtime.cache.ssd_used_bytes` | Its prompt cache on disk (oMLX) |
| `runtime.cache.ssd_capacity_bytes` | That cache's size |
| `runtime.draft.acceptance_ratio` | The share of drafted tokens accepted since the last poll (speculative decoding) |
| `runtime.draft.tokens_per_pass` | Tokens one verify pass emitted since the last poll: the drafted tokens it accepted and the model's own |
| `efficiency` | Decode per verify pass against the chip's published peak, as Diagnostics gives it (see [Decode efficiency](#decode-efficiency)): only while one request decodes alone on this machine and every input is known; else `null` |
| `efficiency.source` | `estimate` |
| `efficiency.draft` | A draft proposes the tokens a pass verifies, so the share is counted per verify pass, the draft's own reads left out |
| `efficiency.verify_passes_per_s` | Verify passes a second: the generation speed over tokens per pass |
| `efficiency.tokens_per_pass` | Tokens a pass emits: the runtime's count with a draft, 1 without |
| `efficiency.bytes_per_pass` | What one pass reads: the weights, the KV cache of the context it decodes and the sequence's fixed state |
| `efficiency.bandwidth_share_ratio` | The share of the chip's published peak those passes read |
| `efficiency.best_bandwidth_share_ratio` | The best share decode has reached for similar work this session while generation kept its usual speed; `null` until there are twenty such samples, and always from `--once`, which watches no session |
| `efficiency.no_draft_ceiling_tok_per_s` | The speed this machine could not exceed without a draft: one token a pass at the chip's full peak |
| `context_left` | How much more context fits, as Diagnostics works it out (see [Diagnostics and runtime setup](#diagnostics-and-runtime-setup)): only while the runtime, its window, the requests in flight and where the room comes from are known; else `null` |
| `context_left.source` | `estimate` |
| `context_left.tokens_per_request` | Tokens each request can still add: the smaller of the two limits below |
| `context_left.requests` | The requests it is for: those in flight, or with none in flight the next one |
| `context_left.by_memory_tokens` | What memory alone leaves each request: the KV room the runtime reports in tokens, else the bytes its memory plan or the machine (less 1 GiB) has left at the KV bytes a token costs; `null` where only the window limits it: a runtime that sized its KV cache for the window when it loaded the model, or a model with no KV cache |
| `context_left.by_window_tokens` | What the context window alone leaves each request: the window less the most context one of them holds |
| `context_left.limited_by` | `memory` or `window`: the limit `tokens_per_request` is (`memory` where the two are equal) |
| `usual_speed` | The usual speed for the work being done, once there is one; else `null` |
| `usual_speed.source` | `estimate` |
| `usual_speed.phase` | `generation` or `prefill` |
| `usual_speed.tok_per_s` | What earlier samples of similar work ran at |
| `usual_speed.slow_below_tok_per_s` | Below this, this work is slow |
| `usual_speed.work` | The work it is for: `1 request · 41.0k context` |
| `usual_speed.relative_change` | This sample's speed against it as a relative change: -0.5 is half as fast, 0 the usual; in a slowdown, against the speed it fell from |
| `unusual` | What else is unusual in this sample, the most telling first: an estimate against each signal's usual value |
| `unusual[].cause` | The signal, by the names `slowdown.cause` uses |
| `unusual[].note` | It beside its usual value, in words |
| `slowdown` | The slowdown or stall under way, or `null` |
| `slowdown.source` | `estimate` |
| `slowdown.phase` | `generation` or `prefill` |
| `slowdown.stalled` | An answer has stopped producing tokens |
| `slowdown.cause` | The reason it is given: `none` (nothing else unusual), `stall`, `paging`, `compression`, `memory_pressure`, `thermal`, `metal_memory`, `queueing`, `concurrent_prefill`, `model_memory`, `disk_io`, `cpu`, `gpu_idle` or `draft_acceptance` |
| `slowdown.reason` | The reason in words, with its usual value |
| `slowdown.usual_tok_per_s` | The speed it fell from |
| `slowdown.lowest_tok_per_s` | The slowest it has been |
| `slowdown.duration_s` | How long it has lasted |
| `slowdown.samples` | Its samples so far |
| `slowdown.stalled_s` | How long a stalled answer has gone without a token |
| `slowdown_ended` | Set on the one sample where a slowdown ended, else `null` |
| `slowdown_ended.source` | `estimate` |
| `slowdown_ended.phase` | `generation` or `prefill` |
| `slowdown_ended.how` | `recovered` (speed came back), `settled` (the slower speed held with nothing else unusual and is now the usual) or `finished` (the request ended while slow) |
| `slowdown_ended.duration_s` | How long it lasted |
| `slowdown_ended.lowest_tok_per_s` | The slowest it was |
| `slowdown_ended.tok_per_s` | The speed it ended at |
| `slowdown_ended.usual_tok_per_s` | The speed it had fallen from |

## Prometheus endpoint

`mlxtop serve` samples without the dashboard and answers HTTP requests on
this machine's loopback interface, for Prometheus, Grafana, the
OpenTelemetry Collector's Prometheus receiver or a script:

```sh
mlxtop serve                                # http://127.0.0.1:10063
mlxtop serve --listen 127.0.0.1:19063 -i 2  # another port, a sample every 2 s
curl -s localhost:10063/metrics | grep tokens_per_second
curl -s localhost:10063/snapshot | jq .verdict
```

| Path | Answer |
| --- | --- |
| `/metrics` | The newest sample in Prometheus's text format (`text/plain; version=0.0.4`, which Prometheus 3 requires) |
| `/snapshot` | The newest sample's [JSON snapshot](#json-output), one line as `mlxtop stream` writes it; 503 before the first sample |
| `/health` | `ok` while sampling is current; 503 and how long there has been no sample otherwise |

Every answer comes from the newest sample, taken every `--interval` seconds
as the dashboard takes them: a request never starts a sample or waits for
one, so a runtime that hangs never holds up a scrape. Sampling is current
while the newest sample is at most three intervals old, and 5 s at least, the
dashboard's rule for NO SAMPLE. Otherwise `mlxtop_up` is 0 and no reading is
written, because Prometheus would store an old reading as the present one.
Rates (`_per_second`) cover the last interval alone, so at the default 1 s
interval a scrape every 15 s sees one second in fifteen; the runtime's
counters (`_total`) count everything.
`--record FILE` works as with the dashboard. `mlxtop serve` stops on Ctrl-C,
SIGTERM or SIGHUP, and ends with an error if sampling stops, which the log
explains.

It listens on loopback only. `--listen` takes a loopback address and a port
(`127.0.0.1:PORT`, `[::1]:PORT` or `localhost:PORT`) and refuses any other,
`0.0.0.0` included. A request must name a loopback host (`localhost`,
`127.0.0.1` or `[::1]`, with any port), so a web page whose name a DNS server
points at 127.0.0.1 (DNS rebinding) gets 403. Only GET and HEAD are answered,
one request at a time; a request head may take 8 KiB and 2 seconds at most;
and there are no CORS headers and no HTML. Any local user or process can
still read the answers, which name the runtime and its model.

To read it from another machine, forward the port over SSH from that machine
and point Prometheus at the near end:

```sh
ssh -N -L 10063:127.0.0.1:10063 you@mac-studio.local
```

```yaml
scrape_configs:
  - job_name: mlxtop
    static_configs:
      - targets: ["127.0.0.1:10063"]
```

### Metrics

Like the JSON, the names are **experimental through 3.4**. Each begins
`mlxtop_` and ends in its base unit (`_bytes`, `_seconds`, `_celsius`,
`_watts`, `_per_second`; `_ratio` is a share from 0 to 1), and the counters
end in `_total`. A reading that is unknown is left out, never written as 0.
Labels are few and bounded: `provider` and `model` on
`mlxtop_llm_model_info`, `limiter` on `mlxtop_bottleneck` and a GPU's index
in `gpu`; never a request id or a process name. Help that begins
`Estimate:` is mlxtop's own judgement, as `source` `estimate` is in the
JSON. A runtime's speed is written under the name of its kind: measured over
the last interval, the runtime's own average, or the last finished
request's, as the screen's GEN, AVG GEN and LAST GEN. To see a figure beside
the model it was measured for, join the info metric:
`mlxtop_llm_generation_tokens_per_second * on(instance) group_left(model) mlxtop_llm_model_info`.

| Metric | Type | Meaning |
| --- | --- | --- |
| `mlxtop_up` | gauge | 1 while sampling is current (the newest sample at most three intervals old, 5 s at least); 0 otherwise, and then no reading is written |
| `mlxtop_sample_age_seconds` | gauge | Seconds since the newest sample was taken |
| `mlxtop_bottleneck` | gauge | The verdict as a state set: one series for each state `verdict.state` names in the JSON, labelled `limiter`; 1 for the verdict's state, 0 for every other |
| `mlxtop_llm_model_info` | gauge | 1, labelled with the runtime found (`provider`) and the model it serves (`model`) |
| `mlxtop_interval_seconds` | gauge | Seconds between samples: the span each rate is measured over |
| `mlxtop_ram_total_bytes` | gauge | All of RAM |
| `mlxtop_ram_available_ratio` | gauge | The share of RAM the operating system reports available |
| `mlxtop_memory_pressure_level` | gauge | The operating system's memory pressure: 0 normal, 1 warning, 2 critical |
| `mlxtop_memory_tight` | gauge | 1 while pressure is raised or little memory is free, so file paging counts against the machine |
| `mlxtop_ram_used_bytes` | gauge | Wired, app and compressed memory: what the RAM meter fills |
| `mlxtop_ram_left_bytes` | gauge | Free memory and file cache: what a model can still grow into |
| `mlxtop_ram_wired_bytes` | gauge | Memory that cannot be paged out |
| `mlxtop_ram_app_bytes` | gauge | Apps' memory |
| `mlxtop_ram_compressed_bytes` | gauge | What the memory compressor holds (macOS) |
| `mlxtop_ram_file_cache_bytes` | gauge | File cache |
| `mlxtop_ram_free_bytes` | gauge | Free memory |
| `mlxtop_gpu_memory_used_bytes` | gauge | What the GPU holds of RAM, where it has no memory of its own |
| `mlxtop_gpu_memory_limit_bytes` | gauge | The GPU's wired-memory limit, where it shares RAM |
| `mlxtop_gpu_memory_left_bytes` | gauge | The room left under that limit; 0 once over it |
| `mlxtop_vram_used_bytes` | gauge | The cards' own memory in use, all cards together |
| `mlxtop_vram_total_bytes` | gauge | The cards' own memory, all cards together |
| `mlxtop_vram_left_bytes` | gauge | The cards' own memory left, all cards together |
| `mlxtop_swap_total_bytes` | gauge | Swap allocated |
| `mlxtop_swap_used_bytes` | gauge | Swap in use |
| `mlxtop_swap_in_bytes_per_second` | gauge | Swapped in, over the last interval |
| `mlxtop_swap_out_bytes_per_second` | gauge | Swapped out, over the last interval |
| `mlxtop_swap_growth_bytes_per_second` | gauge | How fast swap use grows; negative while it shrinks |
| `mlxtop_file_paging_in_bytes_per_second` | gauge | Pages read from disk outside swap (macOS): model loads, and a full Mac's paging with swap off |
| `mlxtop_file_paging_out_bytes_per_second` | gauge | Pages written to disk outside swap (macOS) |
| `mlxtop_memory_compressed_bytes_per_second` | gauge | Memory compressed, over the last interval (macOS) |
| `mlxtop_memory_decompressed_bytes_per_second` | gauge | Memory decompressed, over the last interval (macOS) |
| `mlxtop_published_bandwidth_bytes_per_second` | gauge | Estimate: the peak memory bandwidth the maker publishes for this chip (the Mac's, or the one card) |
| `mlxtop_thermal_pressure_level` | gauge | macOS's thermal pressure: 0 nominal, 1 moderate, 2 heavy, 3 trapping, 4 sleeping |
| `mlxtop_thermal_limited` | gauge | 1 while macOS holds the machine back for heat |
| `mlxtop_cpu_speed_limit_ratio` | gauge | The share of full CPU speed macOS allows (Intel Macs) |
| `mlxtop_hottest_temperature_celsius` | gauge | The hottest temperature measured: thermal zones and GPUs on Linux, GPUs on Windows |
| `mlxtop_system_power_watts` | gauge | What the whole machine draws (macOS) |
| `mlxtop_runtime_processes` | gauge | The runtime's processes |
| `mlxtop_runtime_process_memory_bytes` | gauge | What the runtime holds: its largest process's footprint or all its processes' resident size, whichever is more |
| `mlxtop_runtime_process_peak_bytes` | gauge | Its largest process's lifetime peak footprint (macOS) |
| `mlxtop_runtime_process_growth_bytes_per_second` | gauge | How fast that footprint grows |
| `mlxtop_runtime_process_cpu_cores` | gauge | CPU the runtime's processes use, in cores |
| `mlxtop_runtime_process_disk_read_bytes_per_second` | gauge | The runtime's storage reads (macOS) |
| `mlxtop_runtime_process_disk_write_bytes_per_second` | gauge | The runtime's storage writes (macOS) |
| `mlxtop_llm_up` | gauge | 1 while the runtime's API answers, 0 when it does not |
| `mlxtop_llm_remote` | gauge | 1 when the runtime runs on another machine: the host readings are not its own |
| `mlxtop_llm_telemetry_age_seconds` | gauge | How old the runtime's readings were when the sample was taken |
| `mlxtop_llm_generation_tokens_per_second` | gauge | Generation speed over the last interval |
| `mlxtop_llm_generation_average_tokens_per_second` | gauge | The runtime's own average generation speed, while it reports no live one |
| `mlxtop_llm_last_request_generation_tokens_per_second` | gauge | The last finished request's generation speed, from a completion log or a usage file |
| `mlxtop_llm_prefill_tokens_per_second` | gauge | Prompt reading speed over the last interval |
| `mlxtop_llm_prefill_average_tokens_per_second` | gauge | The runtime's own average prompt reading speed, while it reports no live one |
| `mlxtop_llm_last_request_prefill_tokens_per_second` | gauge | The last finished request's prompt reading speed |
| `mlxtop_llm_prefill_done_tokens` | gauge | Prompt tokens the prefill in flight has read |
| `mlxtop_llm_prefill_expected_tokens` | gauge | Prompt tokens the prefill in flight will read |
| `mlxtop_llm_requests_active` | gauge | Requests running |
| `mlxtop_llm_requests_waiting` | gauge | Requests queued |
| `mlxtop_llm_requests_prefilling` | gauge | Running requests reading their prompt, where the runtime lists its requests |
| `mlxtop_llm_requests_generating` | gauge | Running requests writing their answer, where the runtime lists its requests |
| `mlxtop_llm_context_tokens` | gauge | Tokens the requests writing an answer hold, prompt and output together |
| `mlxtop_llm_model_memory_bytes` | gauge | Memory the runtime's models take |
| `mlxtop_llm_model_memory_ceiling_bytes` | gauge | The most memory the runtime lets its models take |
| `mlxtop_llm_model_size_bytes` | gauge | The loaded model's size (Ollama) |
| `mlxtop_llm_model_offloaded_bytes` | gauge | The part of the model placed outside GPU memory (Ollama) |
| `mlxtop_llm_context_allowed_tokens` | gauge | The context the runtime allows for the model |
| `mlxtop_llm_kv_free_tokens` | gauge | KV-cache room the runtime reports left, in tokens: a pool its requests share, or llama.cpp's running slot nearest its limit |
| `mlxtop_llm_kv_free_bytes` | gauge | KV-cache room the runtime reports left, in bytes: Splash's memory plan, or SGLang's free tokens at its bytes a token |
| `mlxtop_llm_preemptions_total` | counter | Requests the runtime preempted, retracted or suspended for want of KV cache, by its own count since it started |
| `mlxtop_llm_weight_bytes_per_pass` | gauge | Estimate: weight bytes one forward pass reads for a token |
| `mlxtop_llm_bits_per_weight` | gauge | Estimate: bits per parameter a pass reads, scales and biases included |
| `mlxtop_llm_kv_bytes_per_token` | gauge | KV-cache bytes one more token of context costs: the runtime's figure where it reports one, else an estimate from the model's layers |
| `mlxtop_llm_state_bytes_per_sequence` | gauge | Estimate: what a sequence holds whatever its length (linear-attention state, sliding windows, a draft's window) |
| `mlxtop_llm_context_trained_tokens` | gauge | Estimate: the context the model was trained for, as its config.json declares it |
| `mlxtop_llm_draft_weight_bytes` | gauge | Estimate: the draft model's weight bytes as stored, where speculative decoding drafts |
| `mlxtop_llm_mlx_active_bytes` | gauge | MLX's active memory in the runtime |
| `mlxtop_llm_mlx_cache_bytes` | gauge | MLX's cache |
| `mlxtop_llm_mlx_peak_bytes` | gauge | MLX's peak memory |
| `mlxtop_llm_mlx_process_footprint_bytes` | gauge | The serving process's footprint, as the runtime reports it |
| `mlxtop_llm_cache_reuse_ratio` | gauge | The share of prompt tokens served from cache since the runtime started |
| `mlxtop_llm_cache_recent_reuse_ratio` | gauge | The share of prompt tokens served from cache over the last interval |
| `mlxtop_llm_prefix_cache_hit_ratio` | gauge | Prefix-cache hits |
| `mlxtop_llm_prompt_tokens_total` | counter | Prompt tokens the runtime was sent, by its own count since it started |
| `mlxtop_llm_cached_prompt_tokens_total` | counter | Of those, the ones served from its cache |
| `mlxtop_llm_ssd_cache_used_bytes` | gauge | The runtime's prompt cache on disk (oMLX) |
| `mlxtop_llm_ssd_cache_capacity_bytes` | gauge | The size of that cache |
| `mlxtop_llm_draft_acceptance_ratio` | gauge | The share of drafted tokens accepted since the last poll (speculative decoding) |
| `mlxtop_llm_tokens_per_verify_pass` | gauge | Tokens one verify pass emitted since the last poll: the drafted tokens accepted and the model's own (speculative decoding) |
| `mlxtop_llm_decode_verify_passes_per_second` | gauge | Estimate: verify passes a second while one request decodes alone: generation speed over tokens per pass |
| `mlxtop_llm_decode_bytes_per_pass` | gauge | Estimate: what one decode pass reads: the weights, the KV cache of its context and the sequence's state |
| `mlxtop_llm_decode_bandwidth_share_ratio` | gauge | Estimate: the share of the chip's published peak bandwidth decode reads, per verify pass where a draft runs (its own reads left out) |
| `mlxtop_llm_decode_best_bandwidth_share_ratio` | gauge | Estimate: the best share decode has reached for similar work this session, while generation kept its usual speed |
| `mlxtop_llm_decode_no_draft_ceiling_tokens_per_second` | gauge | Estimate: the generation speed this machine could not exceed without a draft: one token a pass at the chip's full peak |
| `mlxtop_llm_context_left_tokens` | gauge | Estimate: tokens each request in flight can still add, or the next with none in flight: the smaller of what memory and the context window allow |
| `mlxtop_llm_context_left_requests` | gauge | Estimate: the requests that context is for: those in flight, or 1 for the next |
| `mlxtop_llm_context_left_by_memory_tokens` | gauge | Estimate: what memory alone leaves each request; left out where only the context window limits it |
| `mlxtop_llm_context_left_by_window_tokens` | gauge | Estimate: what the context window alone leaves each request: the window less the most context one holds |
| `mlxtop_llm_usual_generation_tokens_per_second` | gauge | Estimate: the speed earlier samples of similar generation ran at |
| `mlxtop_llm_generation_slow_below_tokens_per_second` | gauge | Estimate: below this speed, this generation is slow |
| `mlxtop_llm_usual_prefill_tokens_per_second` | gauge | Estimate: the speed earlier samples of similar prompt reading ran at |
| `mlxtop_llm_prefill_slow_below_tokens_per_second` | gauge | Estimate: below this speed, this prompt reading is slow |
| `mlxtop_llm_slowdown_seconds` | gauge | Estimate: how long the slowdown under way has lasted |
| `mlxtop_llm_stall_seconds` | gauge | Estimate: how long a stalled answer has gone without a token |

Each GPU (the Apple GPU of a Mac, or each NVIDIA and AMD card) is labelled
with its index, `gpu`:

| Metric | Type | Meaning |
| --- | --- | --- |
| `mlxtop_gpu_busy_ratio` | gauge | How busy the GPU is |
| `mlxtop_gpu_renderer_busy_ratio` | gauge | How busy the Apple GPU's renderer is |
| `mlxtop_gpu_tiler_busy_ratio` | gauge | How busy the Apple GPU's tiler is |
| `mlxtop_gpu_vram_used_bytes` | gauge | A card's own memory in use |
| `mlxtop_gpu_vram_total_bytes` | gauge | A card's own memory |
| `mlxtop_gpu_temperature_celsius` | gauge | A card's temperature |
| `mlxtop_gpu_throttled` | gauge | 1 while the driver holds a card's clocks down: heat, power brake, hardware slowdown or power cap |

## Configuration

Press `4` to edit these options in [Settings](#settings), or create
`~/.config/mlxtop/config.json` by hand. All fields are optional, and anything
you leave out keeps its built-in default.

```json
{
  "interval": 2,
  "history": 500,
  "omx": {
    "host": "127.0.0.1",
    "port": 8000
  },
  "memory_warn_load": 70,
  "memory_critical_load": 85,
  "gpu_warn_load": 75,
  "gpu_critical_load": 90,
  "gpu_warn_exit": 70,
  "swap_warn_rate": 1048576,
  "swap_critical_rate": 16777216,
  "swap_warn_exit": 2097152,
  "compression_warn_rate": 67108864,
  "compression_warn_exit": 33554432,
  "journal": {
    "gpu": false
  }
}
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `api_key` | string | unset | Key sent to the monitored inference engine when it asks for one; `MLXTOP_PROVIDER_API_KEY` overrides it. Keep the file readable by you only (Settings does) |
| `interval` | integer | 1 | Refresh interval in seconds (1–60) |
| `history` | integer | 300 | Chart/journal history size (20–3600) |
| `omx.host` | string | "127.0.0.1" | oMLX server host |
| `omx.port` | integer | 8000 | oMLX server port; unset, discovered as described in [oMLX telemetry](#omlx-telemetry) |
| `memory_warn_load` | integer | 70 | Unavailable-memory threshold (%) for derived Linux pressure; macOS uses native pressure |
| `memory_critical_load` | integer | 85 | Critical unavailable-memory threshold (%) for derived Linux pressure; macOS uses native pressure |
| `gpu_warn_load` | integer | 75 | GPU load (%) that turns "GPU BUSY" on, shown in orange |
| `gpu_critical_load` | integer | 90 | GPU load (%) reported as "saturated" and shown in red; the Journal clears saturation 10 points lower; never an alarm by itself |
| `gpu_warn_exit` | integer | 70 | GPU load (%) below which "GPU BUSY" clears |
| `swap_warn_rate` | integer | 1 MiB/s | Swap churn that counts as light paging |
| `swap_critical_rate` | integer | 16 MiB/s | Swap rate that counts as thrashing (in and out); critical paging findings turn the paging chart red |
| `swap_warn_exit` | integer | 2 MiB/s | Swap churn below which "PAGING ACTIVE" clears |
| `compression_warn_rate` | integer | 64 MiB/s | Compression churn that raises "COMPRESSION ACTIVE" |
| `compression_warn_exit` | integer | 32 MiB/s | Compression churn below which it clears |
| `alarms` | boolean | true | `false` turns off the critical-condition bell and banner; charts, the assessment and the Journal still show the condition |
| `journal.prompts` | boolean | true | Record a row for each new request |
| `journal.llm` | boolean | true | Record model, status, telemetry and request start/finish changes |
| `journal.queue` | boolean | true | Record requests starting to wait and the queue clearing |
| `journal.memory` | boolean | true | Record memory pressure changes; a move to critical pressure is always recorded |
| `journal.paging` | boolean | true | Record light paging starting and clearing |
| `journal.gpu` | boolean | true | Record GPU BUSY, saturated and eased changes |
| `journal.thermal` | boolean | true | Record thermal warning changes |

Rates are bytes per second; loads are percentages. A `journal` toggle set to
`false` stops new rows of that kind. System rows, assessment findings
(such as HEAVY PAGING, GPU THROTTLED or VRAM FULL) and a move to critical
memory pressure are always recorded.

### Precedence

- **CLI arguments override config file values.** `--interval` and `--history` win
  over `interval` and `history`.
- **Explicit endpoint settings override discovery.** mlxtop reads
  `~/.config/omlx-coding/server.env`, `OMLX_PORT` and oMLX's `settings.json` to
  find a running oMLX server, but a host or
  port you set under `omx` always wins. Each field resolves on its own, so setting
  only `omx.port` keeps the discovered host.
- **Out-of-range `interval` or `history` in the file falls back to the default**
  and is recorded in the diagnostics log, so a typo in `config.json` cannot stop
  mlxtop from starting. The same value passed on the command line is still a hard
  error, because you can see the message straight away.
- **Nonsensical thresholds are clamped, not rejected.** Percentages are held to
  0–100, a critical level is never allowed below its warning level, and a
  hysteresis exit is never allowed above the level that turns the state on, so a
  stray value cannot silently remove a severity band. A config file that fails to
  parse is logged to the diagnostics log and ignored in full.

## Crash diagnostics

Every run writes a small local diagnostic log to:

```text
~/Library/Logs/mlxtop/mlxtop.log        # macOS
~/.local/state/mlxtop/mlxtop.log        # Linux ($XDG_STATE_HOME respected)
%LOCALAPPDATA%\mlxtop\mlxtop.log        # Windows
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

Each sample adds an `event=sample` line of `name=value` fields: memory,
paging, GPU, generation and prefill speed, requests and the runtime's disk
work, then `pressure=` (the memory pressure level), `thermal=`, `power_mw=`
(the whole machine's draw), `draft=` (the share of draft tokens accepted since
the last poll) and `ts=`, the sample's time as a recording gives it. A
reading that was not taken is `na`. New fields are only ever added at the
end.

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

The summary also shows the change from the previous observed request, for
example `Δ +12,768 (+63.8%) vs previous`, when there is one. Prompt load spans
two of the three columns, beside Cache and Queue. It is nine to thirteen rows
high, or seven on short macOS terminals and six with a compact NVIDIA device
table; with no requests yet it is a single line (`No requests yet`, `waiting
for the prompt size`, or `busy outside oMLX`). Enter expands it for a larger
view.
Newest requests stay at the right with fixed spacing, including a single bar.
Every visible bar has a size label beneath it,
such as `12.0k` for 12,000 tokens (`k` means thousands, `M` means millions;
these are token counts, not kilobytes). Each request gets enough horizontal
space to keep its label readable, including at 1×. Older requests remain
available with Shift+↑↓ and Home/End; `#n / m` shows the position while
browsing. Exact selected tokens, cache reuse and the previous-request
comparison stay above the chart, with the local observation time and age for
requests that are no longer live; recent prompt-size statistics sit in the
bottom border.
`LIVE` requires a matching request in a fresh live sample. A request waiting
in the runtime's queue reads `QUEUED` and its bar is hatched. Once absent or
stale, an incomplete request reads `LAST SEEN`; completed request counts read
`REPORTED`.

On a panel that keeps at least two rows of bars, which the Overview's does
at 167×42, a row under the size labels shows the speed each request
generated at (`24.5/s`, or `24.5 tok/s` where the bars are wide enough), so
prompts can be compared by what they cost; a request that has generated
nothing yet shows none. On a panel that keeps four rows of bars besides, the
tokens each request has generated go under that, so the count needs no
subtraction. At 80×24 the panel is too short for either; Enter expands it. Both rows depend on the panel's height
alone, so the bars don't shrink when a request first generates output.

While the selected live request is still prefilling, its output line becomes
a progress bar:

```text
PREFILL ▕████████░░░░░░░▏ 52% · 34,078 / 65,536 · 4m 12s left
```

The time left divides the remaining prompt tokens by the live prefill rate;
narrow panels drop the counts first, then the time left. Progress comes from
oMLX's prefilling rows and Splash's `prompt_processed`; runtimes that do not
report it keep the output line.
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
small panels prioritize it over the clock and optional comparison details.

A completed request whose runtime reported more of its timing adds it to the
same line, each detail shown whole or not at all:
**OUT 512 · AVG 40.0 tok/s · MODEL LOAD 3.2s · PREFILL 812 tok/s**.
`MODEL LOAD` is how long the request waited for the model to load (Ollama's
`load_duration`, LM Studio's `model_load_time_seconds`); a load under a second
is not shown, since Ollama reports a few milliseconds even for a loaded model.
`PREFILL` is how fast the prompt was read, over the tokens the runtime
computed rather than took from its cache (llama-server's `prompt_per_second`,
Ollama's `prompt_eval_duration`); a request that computed fewer than 512 prompt
tokens, one batch, shows none, since fixed costs dominate its time.

Prompt counts, output-speed observations, chart samples and Journal events are
kept in memory for this mlxtop session. After a restart, the prompt panel waits
for new requests; the server's aggregate averages may still be available.

For oMLX this is each generating row's `tokens_per_second`, or distributed
per-request `decode_tps`. These rates cover output generation. Once an oMLX
request disappears between polls, the retained sample is not a final average.
KoboldCpp supplies `last_eval_speed`. A completed Ollama usage record can supply
`eval_count` and `eval_duration` (nanoseconds); their ratio yields output tok/s.
Total request duration and prompt-evaluation duration are not decode timing;
prompt-evaluation duration over the uncached prompt tokens is the request's
`PREFILL` speed. llama.cpp's live requests come from `/slots` (see Provider
endpoints): each answering request's own speed is its output's growth between
samples.


The colored bar chart reads older to newer, with the selected request marked
`▲` beside the size label below the rightmost bar. A single request starts at
the right edge; fixed spacing keeps it there as more requests arrive. Its local
timestamp stays in the headline once it is no longer live. Spacing represents request order, not elapsed time; the selected marker
remains visible during jumps and overflow. When request-specific cache counts are reported, bars stack
green cached tokens below uncached tokens (cyan for live requests, blue for
history). Without a cache count, a solid bar represents the whole prompt and
does not imply zero reuse. `↑` marks a prompt above the display scale. Changes
in prompt size remain numeric comparisons, without warning marks above bars.
Bar heights use eighth-cell precision and do not change with cache availability.
Where a partial cell cannot show both segments and empty space, it uses the
dominant segment color. Exact selected values remain in the summary and cache
readout. Labels accompany color cues.

The bottom line shows the median of up to eight contiguous observations
ending at the selection, from the same provider/model, and how many it covers
(`MEDIAN 12,000 tokens · RECENT 7`); the bars show the range. Moving
back through history excludes later requests from those statistics. These are
prompt sizes in tokens, not health or latency assessments. The headline keeps
the exact selected prompt size and its observation age visible.

Request cache reuse uses the cache chart's bands for prompts of at least 4,096
tokens: green from 50%, orange from 20%, red below 20%. Smaller prompts stay
cyan. Low reuse on a cold request is expected;
a low percentage by itself does not establish a cache problem.

The chart uses an **automatic zero-based token scale** derived from the visible
requests. A history of roughly 800-token prompts therefore fills a useful
range near 1,000 tokens. Larger prompts and history zoom update that range;
the headline always gives the selected request's exact count. This is a display
range, not a context limit or pressure threshold.
Cache counts appear only when reported for the selected request. Aggregate cache
statistics and prefill rates are never substituted for request-specific data.

Comparisons are with the previous observed request (`vs previous`), and are
suppressed across changes of provider or model. They do not establish conversation membership or explain
latency on their own. Request identifiers and sampled output counts remain in
Journal instead of occupying an Overview table.

Up to 240 distinct requests are retained. Repeated polls update an existing
observation, and past requests stay available. Use **Shift-↑ / Shift-↓** or
**PgUp / PgDn** to browse, **Home** for latest, **End** for oldest, and **r** to reset history.

## Operator charts

| Chart | Form | Reading |
| --- | --- | --- |
| memory | what RAM has left over time, in each sample's pressure colour (occupancy area without composition counters) | PRESSURE; a GPU meter (wired against the GPU limit) and a RAM meter (in use, cache, what is left) |
| compression | bars from zero, axis at least the warning rate | stored / compressed bytes and ratio, compressing / decompressing rates, share of RAM |
| paging / I/O | bars from zero, axis at least the warning rate | page-in / page-out rates, swap capacity bar |
| generation, prefill | bars from zero | live tok/s, or a labeled `LAST` reading |
| GPU | area on 0–100% | utilization and load state |
| cache | bars on 0–100% | interval hit %, reuse since start and prefix hit |
| queue | two series on a shared count axis; `●`/`○` marks when compact; a list of running requests while the runtime reports each one's speed | running and waiting counts |
| prompt load, first token | one bar per request | the selected request |

All time-series panels share a visible sample window and zoom in Overview. The
window in the top bar, such as `window 46s`, is the rolling history span, not
elapsed runtime. The span stays fixed while new samples enter on the right.
Resize the terminal, expand a chart or change zoom to change the visible span.

GPU and paging samples retain green/orange/red value bands.
GPU defaults are green below 75%, orange from 75% and red from 90%; these are
load bands, not proof of a bottleneck. Configured thresholds apply consistently
to numeric readings, traces and device meters. Percentage axes stay at 0–100;
all other axes use the real measured unit and fit the visible data.

**Queue** plots active requests in cyan and waiting requests in orange on one
shared, labeled zero baseline. A compact queue keeps its counts and adds a
mark per request: `●` running, `○` waiting, `+` when they do not fit. A white `═` marks equal measured counts. Overlapping connectors merge into continuous muted lines and junctions.
Muted `≈` marks unequal values that coincide at terminal resolution. Exact
counts remain visible in the panel. Both series share an automatic request-count scale based on their visible
maximum. Idle zeros are valid; stale, missing and client-reported values
produce gaps. Queue length is a demand signal, not a latency measurement.

When the runtime reports each running request's own decode speed (oMLX,
mlx-serve, and oMLX through Splash), the Overview queue lists the running
requests in its own rows instead, one per line: the end of the request's id,
the tokens it has generated and its speed, such as `8f31a2 1.7k · 41.0 tok/s`.
Compare a row with what that request's client measures; the generation panel
reports the whole server. A request without a speed yet, or one the runtime
can't measure on its own (Splash with two requests running), shows `—` and
keeps its row. When the rows run out, the count reads `active 3 · +1 more`;
the waiting count moves to the border when it has no row. The panel keeps
its size, the history returns when nothing is running, and an enlarged queue
always shows the history.

**First token** appears only after an explicit client timing is supplied in the
existing usage JSONL envelope:

```json
"timings": {"time_to_first_token_ms": 1250, "output_tokens_per_second": 40.0}
```

`output_tokens_per_second` is an optional finite, nonnegative completed-request
output average. It does not change the live generation chart. The envelope may
also carry `prompt_tokens_per_second`, the prompt's speed over the tokens the
runtime computed, and `model_load_ms`, how long the request waited for the
model to load; prompt load shows them as `PREFILL` and `MODEL LOAD`. A record
of any runtime may instead carry llama-server's own `timings` as the client
received them (Splash and proxies send the same shape): `predicted_per_second`
is read as the output speed, `prompt_per_second` as the prompt speed and
`cache_n` as the cached prompt tokens.

Use a nonnegative integer measured from request dispatch to the first generated
token. Add this field alongside `provider`, `request_id`, `observed_at` and
`usage` in a complete record. The chart uses one column per observed request at 1×,
shows missing timings as gaps, and automatically scales its millisecond axis
to the visible requests; bars have eighth-block tops. Its latest measured value
is labeled REPORTED with observation age.
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
| `ollama` | 11434 | `/api/ps`; `POST /api/show` once per model (read-only, see below) |
| `lmstudio` or `llmster` | 1234 | `/api/v1/models`, fallback `/api/v0/models`, then `/v1/models` |
| `vllm` | 8000 | `/metrics` |
| `sglang` | 30000 | `/metrics` (requires `--enable-metrics`) |
| `llama.cpp` | 9931, then 8080 | `/metrics` (requires `--metrics`), `/props`, `/v1/models`, and `/slots` only while a request runs; builds from October 2026 listen on 9931 |
| `koboldcpp` | 5001 | `/api/extra/perf` |
| `mlx-serve` | 11234 | `/metrics` (requires `--metrics`), `/metrics.json` |
| `splash` | 8000 | `/status` |
| `tensorfold` | 8080 | `/metrics`, `/v1/models` |
| `strata` | 8080, then 8095 | `/metrics?format=prometheus`, `/v1/models`; 8095 is a bare `strata serve` |
| `mlx-lm`, `localai` | 8080 | `/v1/models` |
| `jan` | 1337, then 6767 | `/v1/models`; the desktop app serves on 1337, `jan serve` on 6767 |
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
HTTPS certificates are verified. Set the API key in Settings, or
`MLXTOP_PROVIDER_API_KEY` in the environment, when the server requires bearer
authentication. Remote credentials also require
`MLXTOP_ALLOW_REMOTE_AUTH=1`, preserving the existing authentication opt-in.
Keys are never printed or stored
in usage reports. Redirects and environment proxies are disabled; credentials
are sent only to the configured server. Requests time out, response sizes are
bounded and failures back off. Cached readings keep their original age and
become stale after five seconds. Invalid URLs or rejected authentication leave
telemetry unavailable; there is no unauthenticated fallback to another server.

oMLX retains its existing endpoint and login configuration; these provider
variables apply to the other adapters. API details appear in **Processes** and
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
- **llama-server:** reads `/metrics` first, when the server starts with
  `--metrics`: aggregate average rates, active/deferred queue counts, prompt
  tokens served from its cache (the KV cache panel's hit rate), how many slots
  each decode step served on average, and its speculative-decoding counters.
  It reads `/slots` only while `/metrics` counts a request in flight. A server
  started with `--sleep-idle-seconds` unloads its model when idle and loads it
  again for the next request; `/health`, `/props`, `/models` and `/metrics`
  neither wake it nor count as activity, while every `/slots` read does both. So
  mlxtop never wakes it, and an idle server it watches can fall asleep.
  Without `/metrics`, `/slots` is the only window on requests: mlxtop reads it
  every sample while `/props` says the server is awake, asks `/props` first
  after any pause, and stops while the server sleeps, rechecking every five
  seconds; those reads count as activity, so the server sleeps only if it was
  asleep before mlxtop looked. A sleeping server reads as idle, with
  `Asleep` in Diagnostics, never as an error; a server whose `/props` says
  `/slots` is off (`--no-slots`) is never asked for it, since asking would wake
  it.
  Output counts sum all active slots;
  if any active slot omits its output count, the total stays unavailable.
  `next_token` is read both as an object and as the per-sequence list newer
  builds return. While requests run, the generation rate is live: the growth of
  each running request's output between samples, counted only when the same
  slot runs the same task in both, so a finished or newly started request never
  reads as speed. A request also counts only once it has started decoding, so a
  prompt still being processed never reads as 0 tok/s. The server's `/metrics` rates average
  finished requests, read zero through a long first answer, and return when the
  server is idle.
  Each request that has read its prompt and started answering gets its own row
  in prompt load and Queue, named by its task number as the server's log names
  it (with `-1`, `-2` after a restart seen while watching): its whole prompt
  (the tokens taken from the cache plus those computed), its cached share,
  its output and its own speed. A prompt still being read has no row, since
  `/slots` does not say how long it is; Diagnostics says how many of its
  tokens are in so far and how many came from the cache. Diagnostics also
  gives how much more context fits in each slot's window (with a unified KV
  cache, llama-server's default with automatic slots, requests share one pool,
  so two at once each have less), the slots and how many shared a decode step
  since the server started, and speculative
  decoding's tokens per verify pass and accepted share since it started.
  Tokens per verify pass for the requests that finished since the last
  sample come from the growth of `spec_decode_num_accepted_tokens_total`
  over `spec_decode_num_drafts_total`; llama-server adds a request's drafts
  when it finishes and prints six significant digits, so on a server that
  has counted millions the figure waits until enough passes are counted to
  read it to within a hundredth. Use the usage-file integration below for
  exact completion usage.
- **Ollama:** `/api/ps` supplies loaded models and summed resident VRAM, and for a
  single model its architecture and runner (`llama on llama.cpp`, `on MLX`), its
  quantization and the context it is loaded with. Residency does not establish
  request activity. For that model mlxtop also asks `POST /api/show` once per model
  digest: a read-only request that reads the model's manifest and file header and
  loads nothing. Its parameter count, trained context, experts and, for a GGUF model
  of standard attention, its layers, KV heads and head dimension become the
  [model's facts](#model-facts). A safetensors model on the MLX runner names no head
  dimension or layer kinds there, so those stay unknown, as does the KV format, which
  Ollama does not report. An answer that cannot be read is asked again a minute later,
  never on every poll, and a cloud model is never asked about. The native completion
  recorder preserves decode speed and cached prompt tokens; total request duration is
  not used as decode time or TTFT.
- **LM Studio:** native v1 supplies loaded instances; for a single instance also its
  quantization, the experts it runs per token, the context it is loaded with and the
  context the model supports (as model facts), its format, the size of its weights on
  disk and how many requests it runs at once. v0 supplies loaded model identity. The
  final fallback supplies an available-model catalogue. Download size is never
  reported as allocated RAM. Native completion stats supply output speed and
  explicitly reported first-token time through the usage recorder.
- **vLLM/SGLang:** Prometheus polling sums active/waiting requests and computes
  server-wide generation/prompt-token rates from successive counter samples.
  These are sampled server rates, not per-request averages. The first sample,
  counter resets, changed series and gaps longer than five seconds produce no
  rate. vLLM supplies interval and cumulative prefix-token cache reuse (from
  `gpu_prefix_cache_*` on vLLM 0.9.1 and earlier). SGLang's reuse is its cached
  prompt tokens over all prompt tokens. SGLang adds a request's prompt and output
  tokens to those counters only when it finishes, so its decode rate comes from
  `sglang:realtime_tokens_total{mode="decode"}`, which it advances on every decode
  step: live, and zero through a stall. An SGLang without it falls back to its
  `gen_throughput` gauge (the speed of its last few dozen decode steps), and one
  with neither to completion averages, kept out of live rate charts and slowdown
  detection. Its prefill rate stays a completion average.
  Processes shows maximum reported KV occupancy across engines and cumulative mean
  TTFT. Aggregate latency is never inserted into individual request history.
  No per-request prompt counts or IDs are inferred from aggregate counters.
  Diagnostics adds what each engine counts about its own work, each only when it
  reports it:
  - **Since the last poll:** the mean queue wait and time between tokens of the
    work done (from the `_sum` and `_count` of `request_queue_time_seconds` and
    `inter_token_latency_seconds`, or SGLang's `queue_time_seconds`); vLLM's share
    of draft tokens accepted; SGLang's cached tokens evicted.
  - **Speculative decoding:** the tokens a verify pass emits, the draft tokens
    accepted plus the target's own: vLLM's accepted tokens per draft plus one,
    SGLang's `spec_accept_length` while it decodes. Whether a draft is configured
    at all is a model fact: vLLM registers its draft counters exactly when it has
    one, and SGLang names its draft once it has decoded.
  - **KV cache:** the room left in tokens, on the engine with the least: for vLLM
    each engine's capacity from `vllm:cache_config_info` (`kv_cache_size_tokens`,
    else blocks × block size) less what its usage gauge holds; for SGLang free
    slots plus cached prefixes it would evict. SGLang also gives the bytes a token
    costs (its KV pool over the tokens it holds; not for a hybrid sliding-window or
    Mamba pool), so the room in bytes, and the context it allows. Then the requests
    preempted (vLLM) or retracted (SGLang) for KV room since the server started.
    vLLM's KV format (`cache_dtype`) is a model fact unless it is `auto`.
  - **SGLang's time:** over the last half-minute, the share each scheduler spent
    with the GPU running forward passes (only with
    `SGLANG_ENABLE_METRICS_DEVICE_TIMER=1`), idle with no work at all, and on its
    own CPU time across threads. SGLang credits these about once a second and an
    idle stretch when work resumes, so a poll credited more than its own length is
    left out.
  A counter that falls (a restart) gives no figure for that interval, never a
  negative or inflated one.
- **mlx-serve / sushi:** polls `/metrics` (start mlx-serve with `--metrics`; sushi
  serves it by default; set
  `MLXTOP_PROVIDER_API_KEY` if it has an API key) and `/metrics.json` for the model and
  one entry per running request (prompt, cached and output counts; cache-only sessions
  are skipped). Each running request's own speed is read the same way from its
  `generated_tokens`, which counts the tokens its client receives; a request
  reused under the same id with a different prompt starts over, and a session still
  prefilling, or one without a `request_id`, has none. Generation rate keeps the points where the runtime's
  `mlx_serve:generation_tokens_live` (or `sushi:generation_tokens_live`)
  advanced and divides across a span of at least six seconds, because the server
  refreshes that gauge once every two seconds and a one-second delta reads 0 or twice
  the true speed; the last real speed stays shown, not live, for four seconds after the
  gauge stops, then the rate reads a live zero. Prefill rate is the growth of
  the runtime's `mlx_serve:prefill_tokens_live` (or `sushi:prefill_tokens_live`) since the prefill was first seen, over the time to its
  latest advance, because the gauge moves in coarse chunks; it shows nothing until the
  first chunk lands and is live only while a prefill is running. While it runs, the
  prefill panel's footer shows `prefill_tokens_live / prefill_tokens_expected` with the
  percentage. The last speed stays
  shown, not live, for three seconds, replaced by the finished request's own speed when
  it ends inside that time. A server without `/metrics.json` is asked once per restart. Cache reuse is tokens
  restored from the prefix cache over all prompt tokens. Sampling follows the vLLM rules.
- **Splash:** polls `/status` (set `MLXTOP_PROVIDER_API_KEY` when the server runs with
  `--api-key` or `SPLASH_API_KEY`, and `MLXTOP_PROVIDER_PORT` to match `splash serve --port`).
  It is detected by its Metal engine process, `…/engine/splash`. Each request in prefill,
  decode or waiting for a grammar mask is listed with its prompt and generated counts;
  queued requests and those waiting for memory or a prefix count as waiting. Generation
  rate is the growth of `metrics.decode_output_tokens` between captures; a busy engine
  answers with its last snapshot, so each capture is dated by `transport.status_age_ms`
  and a repeated one keeps the previous rate. Prefill speed is prefilled tokens over the
  GPU time of the prefill commands (`metrics.prefill_wall_ms`), because both move once per
  chunk; while a prompt is still in prefill the last chunk's speed stays live, and it
  shows nothing until the first chunk lands. Once nothing is prefilling the last speed
  stays shown, not live, for three seconds. Cache reuse is `cache.reused_tokens` over
  reused plus prefilled tokens. An engine restart or a gap longer than 30 seconds
  produces no rate. Memory is Splash's Metal allocations (`memory_actual`), not process
  RSS, against `memory_governor.limit_bytes`. A request decoding alone gets its own
  output speed from the decode tokens over decode time; concurrent requests share those
  counters and keep their last observed output instead. A request that leaves the
  engine becomes a completed record for prompt load and the Journal, with an exact
  final output when nothing else was running. With a draft, tokens per verify pass are
  the draft tokens accepted per pass plus the one each pass adds, between captures:
  `metrics.accepted_draft_tokens` over the passes in `scheduler.decode_batches_by_width`,
  a batch of width w being w passes, as Splash's log counts its `tok/cycle`. Fewer than
  eight passes since the last capture give none. While one request writes alone, prompt
  load shows them beside its output (`OUT 1,200 · 7.8 tok/pass`). The memory plan gives
  the [model's facts](#model-facts) (KV layers, heads and format, KV per token, the state
  each request holds, the context allowed and trained) and the KV room left: the plan's
  KV pages less those running requests hold, pages that only hold cached prefixes
  counting as room. Requests suspended for memory (`cache.resource_suspensions`) are
  counted. The details line leads with problems
  (failed or cancelled requests, Metal health, capacity failures, requests waiting for
  memory, gated SSD cache writes, thermal state, pressure), then TTFT and inter-token
  p50/p95 and Metal headroom. Splash 1.6 also counts what held work up, and the details
  line says what did since the last capture: requests waiting for more memory and
  Metal pipelines compiled while serving among the problems, then what paused decoding
  (`decode paused 3× for prefill (420 ms)`), what held each first token (`first token
  5.1 s: 3.4 s behind another prefill, 1.2 s own prefill`) and KV moved to or read from
  the SSD. Splash reports no per-request cached tokens or TTFT.
- **TensorFold:** detected by the `tensorfold` or `tensorfold-native` executable,
  or selected with `MLXTOP_PROVIDER=tensorfold`. Reads `/metrics` for active/queued requests,
  throughput, prefix-cache reuse, KV occupancy and model memory
  (device memory on CUDA, the process footprint on a Mac);
  `/v1/models` supplies model names. The decode rate is the growth of the tokens its
  running streams hold (`generation_tokens_running`) plus its finished total, so a
  request ending between two polls neither adds nor loses tokens: it is live and needs
  two samples. A TensorFold without that gauge, and the prefill rate always, advance
  only when requests finish; those rates are labeled as averages and excluded from live
  throughput charts and slowdown alarms. Diagnostics adds the tokens a decode round of
  the requests finished since the last poll emitted with MTP drafting (whether it
  drafts is a model fact, known once requests have finished), the requests that gave a
  lane up to a later one, and the requests its admission limits refused. Request history and individual
  prompt/cache counts require the client usage recorder. No dashboard flag is required.
  For an authenticated local server, replace `/path/to/api-key` with the key file
  configured for that server:

  ```sh
  MLXTOP_PROVIDER=tensorfold \
    MLXTOP_PROVIDER_API_KEY="$(cat "/path/to/api-key")" mlxtop
  ```

  Set `MLXTOP_PROVIDER_PORT` for a custom port or `MLXTOP_PROVIDER_URL` to the server
  root URL (without `/v1`). The existing remote-auth restrictions also apply.
- **Strata:** detected by the `strata` or `strata-engine` executable,
  or selected with `MLXTOP_PROVIDER=strata`. Reads `/metrics?format=prometheus` for active and waiting
  queues, KV occupancy, prefix-cache reuse, mean first-token time and engine GPU memory;
  `/v1/models` supplies model names beside the metric labels. Strata's own `strata:live_tok_s`
  and `strata:live_prefill_tok_s_mean` gauges drive live decode and prefill speeds; without
  them the completion counters give averages that are excluded from live throughput charts
  and slowdown alarms. Request history and individual prompt/cache counts require the client
  usage recorder. No dashboard flag is required. For an authenticated local server, replace
  `/path/to/api-key` with the key file configured for that server:

  ```sh
  MLXTOP_PROVIDER=strata \
    MLXTOP_PROVIDER_API_KEY="$(cat "/path/to/api-key")" mlxtop
  ```

  Set `MLXTOP_PROVIDER_PORT` for a custom port or `MLXTOP_PROVIDER_URL` to the server
  root URL (without `/v1`). The existing remote-auth restrictions also apply.
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
`localai`, `vllm`, `sglang`, `jan`, `gpt4all`, `mlx-serve`, `splash`, `tensorfold`, and `strata`.

Copy only usage counters from the response, with the required envelope above:

| Response format | Counter fields |
| --- | --- |
| OpenAI-compatible usage, including MLX-LM | `usage.prompt_tokens`, `usage.completion_tokens`, optional `usage.prompt_tokens_details.cached_tokens` |
| Responses-style usage | `usage.input_tokens`, `usage.output_tokens`, optional `usage.input_tokens_details.cached_tokens` |
| Ollama native | `prompt_eval_count`, `eval_count`, optional `prompt_eval_cached_count`; decode speed from `eval_count / eval_duration`, prompt speed from the uncached prompt tokens over `prompt_eval_duration`, model load from `load_duration` (all nanoseconds) |
| LM Studio native | `stats.input_tokens`, `stats.total_output_tokens`, `stats.tokens_per_second`, `stats.time_to_first_token_seconds`, `stats.model_load_time_seconds` (sent only after a load); `model_instance_id` is accepted as the model |
| llama-server `timings`, from any runtime | `predicted_per_second`, `prompt_per_second`, `cache_n` |

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

## Overhead

mlxtop's own process uses about 1% of one core and 8 MB of memory. Most of
its cost is in the short-lived commands it runs each sample (`ps`, `ioreg`,
`sysctl`, `vm_stat`, `memory_pressure`, `date`): measured on an Apple M4 with
about 800 processes, all of it together came to 8–9% of one core at the
default one-second interval, more than half of it `ps` listing every process.
The cost falls with the interval: about 4–5% at `-i 2` and under 2% at
`-i 5`. Activity Monitor shows those commands, not mlxtop, so its own row
looks smaller than the whole.

## Data sources and privacy

mlxtop reads macOS counters from `sysctl`, `memory_pressure`, `vm_stat`,
`ioreg`, `notifyutil`, `pmset` and `ps`. On Linux it reads `/proc/meminfo`, `/proc/vmstat`,
`/proc/pressure/memory`, `/sys/class/thermal`, `nvidia-smi` (when present:
the core counters and clock-limit reasons each poll, power, clocks and links
every five seconds, and per-process GPU memory where described above),
the amdgpu files under `/sys/class/drm` and the system `pci.ids` (when present)
and `ps`. On Windows it reads memory, commit charge and the process list
through the Windows APIs (the `sysinfo` crate) and `nvidia-smi` when present. It reads configured provider endpoints and local logs only for
supported adapters. For a model oMLX serves on this machine it reads the
model's `config.json`, the headers of its safetensors shards (never the
weights) and oMLX's `model_settings.json`, once per model and change. Besides
oMLX's admin login, the one runtime request that is not a GET is Ollama's
read-only `POST /api/show`, sent once per model. mlxtop never asks for these routes, which change a runtime's state or generate
text: SGLang's `/health_generate`, `/flush_cache` and its profiler, memory and
session routes, llama-swap's `/unload` and `/upstream/…`, KoboldCpp's
`/api/extra/shutdown`, Foundry Local's load and unload routes and exo's benchmark.
The SGLang adapter asks for `/metrics` alone, never `/health`, which generates a
token by default. The
application does not track how it is used, upload collected metrics, modify model
state or send synthetic inference requests.
`--once --json` and `mlxtop stream` write only to standard output, and
`mlxtop serve` answers only on the loopback interface, though any local user or
process can read what it answers; none of them holds process names, command
lines or process ids.

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
./scripts/cicd.sh --release v3.1.0 --host example.local --user deploy
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
