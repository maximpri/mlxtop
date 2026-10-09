# mlxtop

**A `top` for your local LLM on Mac and Linux.**

See which models are running, how much memory they use, and how busy your
GPUs are. Linux NVIDIA systems show each card's utilization, VRAM and temperature
in Overview, with `[` / `]` navigation for larger GPU sets. With oMLX, vLLM or
SGLang, you can also follow generation speed and request activity as your model
responds.

![mlxtop 2.1.1 Overview on Apple Silicon: memory pressure with RAM composition, compression and paging first, then prompt load and a compact throughput row, colored by threshold](docs/screenshots/overview.png)

[Try it](#try-it) · [Runtime support](#runtime-support-and-limitations) ·
[User guide](docs/USER_GUIDE.md) · [Changelog](CHANGELOG.md) · [Report a bug](https://github.com/maximpri/mlxtop/issues)

**mlxtop 2.1.1** is the current stable release for Apple Silicon macOS and
Linux (x86_64 and ARM64). [Download 2.1.1](https://github.com/maximpri/mlxtop/releases/tag/v2.1.1)
or use the installer below.

## What's new in 2.1

- **Host resources first:** memory pressure with RAM composition (wired, app,
  compressed, cache, free) and the GPU wired limit, a new macOS compression
  chart and paging lead Overview; prompt load follows and throughput stays
  compact. Panels share one aligned grid.
- **A clear verdict:** the assessment says `Healthy · no bottleneck` when
  nothing needs action and names the next check when something does. Press
  `d`, or run `mlxtop doctor`, for evidence and runtime setup.
- **Threshold colors everywhere:** every chart is graded green/yellow/red by
  its configured thresholds; generation and prefill compare with their rolling
  baseline. Charts, assessment and Journal agree.

## What's new in 2.0

- **Eleven runtimes:** oMLX, Ollama, LM Studio, llama.cpp, KoboldCpp, MLX-LM,
  LocalAI, vLLM, SGLang, Jan and GPT4All. The [runtime table](#runtime-support-and-limitations)
  distinguishes live metrics, model inventory and client-reported usage.
- **Remote monitoring:** configured HTTP/HTTPS endpoints and bearer authentication,
  with verified HTTPS certificates and explicit opt-in for remote credentials.
- **Clearer charts:** synchronized rolling windows, linked time-series zoom,
  readable queue crossings and explicit `window 34s` labels. Resident memory
  stays cyan; pressure severity has its own label. Process-memory readings live
  in Top and text reports.
- **Accurate request history:** the counters-only recorder preserves native
  output speeds, cached tokens and explicit first-token timing. Completed
  results remain separate from live serving metrics.

### Upgrading from 1.x

Run the installer again and restart any open mlxtop session. Existing
`~/.config/mlxtop/config.json`, oMLX settings and usage JSONL files remain
compatible. The dashboard monitors one selected server per session; use
`MLXTOP_PROVIDER` to choose it. New runtimes need their monitoring APIs enabled,
and some require the [client usage recorder](docs/USER_GUIDE.md#client-reported-usage-file)
for request statistics.

## Try it

You’ll need an Apple Silicon Mac or a Linux machine and a terminal with
Unicode and color support.
The macOS binary targets macOS 11 or later. The
[latest release notes](https://github.com/maximpri/mlxtop/releases/latest)
list the versions each release was tested on.

[**Download the macOS disk image (.dmg)**](https://github.com/maximpri/mlxtop/releases/latest)
from the latest release. Open it, double-click **Install mlxtop.pkg**, and follow the installer. Then
open Terminal and run `mlxtop`. This installs in `/usr/local/bin` and requires
an administrator account. The package is unsigned and not Apple notarized.

### Install from Terminal

To install in your home directory without sudo, on macOS (Apple Silicon) or
Linux (x86_64 or aarch64):

```sh
curl -fsSL https://raw.githubusercontent.com/maximpri/mlxtop/main/scripts/install.sh | sh
```

The installer picks the download for your system, checks its SHA-256 checksum
and places `mlxtop` in `~/.local/bin`. No Rust toolchain or sudo is needed.
Linux downloads are static binaries with no runtime dependencies. Then run:

```sh
~/.local/bin/mlxtop
```

Add `~/.local/bin` to your `PATH` to run it as `mlxtop` from any terminal.
The installer uses the latest release; to pin one, set `MLXTOP_VERSION`, for
example `curl -fsSL … | MLXTOP_VERSION=2.1.1 sh`.

## Diagnose a connection or missing reading

Press **`d`** in the dashboard to inspect the current assessment, the selected
runtime and endpoint, authentication status, supported measurements, and setup
instructions. The panel is read-only and uses the current samples. Overview also
shows a compact assessment and a suggested check above the charts.

For the same diagnostic information without a terminal UI:

```sh
mlxtop doctor
MLXTOP_PROVIDER=ollama mlxtop doctor
```

`doctor` takes two samples using the configured interval. It returns exit code
1 for invalid configuration, incomplete host counters, or a failed primary runtime
connection. No detected runtime, missing optional metrics, and resource pressure
are informational. The [diagnostics guide](docs/USER_GUIDE.md#diagnostics-and-runtime-setup)
explains partial connections and client recording.

## Configuration

Create `~/.config/mlxtop/config.json` to customize mlxtop. All fields are optional,
and anything you leave out keeps its built-in default.

```json
{
  "interval": 2,
  "history": 500,
  "omx": {
    "host": "127.0.0.1",
    "port": 8080
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
  "compression_warn_exit": 33554432
}
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `interval` | integer | 1 | Refresh interval in seconds (1–60) |
| `history` | integer | 300 | Chart/journal history size (20–3600) |
| `omx.host` | string | "127.0.0.1" | oMLX server host |
| `omx.port` | integer | 8080 | oMLX server port |
| `memory_warn_load` | integer | 70 | Unavailable-memory threshold (%) for derived Linux pressure; macOS uses native pressure |
| `memory_critical_load` | integer | 85 | Critical unavailable-memory threshold (%) for derived Linux pressure; macOS uses native pressure |
| `gpu_warn_load` | integer | 75 | GPU load (%) reported as "busy" and shown in yellow |
| `gpu_critical_load` | integer | 90 | GPU load (%) reported as "saturated" and shown in red; considered in slowdown correlation; never an alarm by itself |
| `gpu_warn_exit` | integer | 70 | GPU load (%) below which "GPU BUSY" clears |
| `swap_warn_rate` | integer | 1 MiB/s | Swap churn that counts as light paging |
| `swap_critical_rate` | integer | 16 MiB/s | Swap rate that counts as thrashing (in and out); critical paging findings turn the paging chart red |
| `swap_warn_exit` | integer | 2 MiB/s | Swap churn below which "PAGING ACTIVE" clears |
| `compression_warn_rate` | integer | 64 MiB/s | Compression churn that raises "COMPRESSION ACTIVE" |
| `compression_warn_exit` | integer | 32 MiB/s | Compression churn below which it clears |

Rates are bytes per second; loads are percentages.

### Precedence

- **CLI arguments override config file values.** `--interval` and `--history` win
  over `interval` and `history`.
- **Explicit endpoint settings override discovery.** mlxtop reads
  `~/.config/omlx-coding/server.env` to find a running oMLX server, but a host or
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

## Usage

You should see live memory and GPU readings as soon as the dashboard opens.
It works without a model running, so you can look around before starting one.
Once mlxtop detects a supported runtime, it adds the available model information.
oMLX provides the most detail, including response speed and request activity.

### Watch it during a conversation

1. Leave mlxtop open and send a message to your local model. Any short prompt
   will do, such as “Explain how a rainbow forms in three sentences.”
2. Watch Overview while the model responds. You’ll see memory and GPU activity.
   With live oMLX data, you’ll also see how fast it processes your prompt and
   writes the answer.
3. Press `2` to inspect model processes or `3` to open the event journal.
   Press `q` when you’re done.

The numbers will depend on your Mac, model, and prompt. A dash in the dashboard
means a measurement isn’t available. Response speed requires a runtime that
reports it.

For a text report you can use in a terminal or over SSH:

```sh
~/.local/bin/mlxtop --once
```

This prints memory, paging, GPU, and available model information, then exits.
If you used the macOS installer or Cargo, use `mlxtop --once` instead.

You don’t need an account or a cloud API key. Use whatever local model you
already have running. mlxtop doesn’t download models or send prompts for you.
Installation needs internet access to download the release, or the source and
Rust dependencies if you build it yourself.
If your oMLX server requires a key, follow the
[runtime setup guide](docs/USER_GUIDE.md#omlx-telemetry).

### Views and controls

Use mlxtop to see whether a slow reply coincides with a busy GPU or your Mac
moving memory to disk. It’s also useful for watching memory use as you try a
larger model or work through a longer conversation.

| View | What it shows |
| --- | --- |
| Overview | Memory pressure and paging, with model status, token rates, prompt history, GPU, queue activity and recent events |
| MLX Top | Running model processes and the resources they use |
| Journal | Request activity and changes in resource use during the session |

Here’s the Journal during an oMLX session:

![mlxtop Journal showing timestamped model requests, queue changes, and GPU events](docs/screenshots/journal.jpg)

The Overview screenshot uses an illustrative oMLX fixture; the Journal shows
an oMLX session. These numbers illustrate the display and aren’t benchmarks.

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Open Overview / MLX Top / Journal |
| `Tab` / `Shift-Tab` | Next / previous view |
| Arrows / click | Select a chart in Overview |
| `p` / `Space` | Pause or resume sampling |
| `+` / `-` / mouse wheel | Zoom the selected chart’s history |
| `Enter` / `Esc` | Enlarge / restore a chart |
| `{` / `}` | Change refresh interval |
| `a` | Acknowledge a critical system alarm |
| `[` / `]` | Select an NVIDIA GPU in Overview and reveal additional cards |
| `d` | Open diagnostics and runtime setup guidance |
| `?` | Show help |
| `q` | Quit |

The [user guide](docs/USER_GUIDE.md#controls) covers process filtering, sorting,
and journal navigation.

<details>
<summary>Build from source</summary>

If you prefer to build it yourself, you’ll need Rust 1.88 or newer with Cargo
and Git:

```sh
git clone https://github.com/maximpri/mlxtop.git
cd mlxtop
cargo install --path . --locked
mlxtop
```

If your shell can’t find `mlxtop`, run `~/.cargo/bin/mlxtop` or add
`~/.cargo/bin` to your `PATH`.

</details>

## Runtime support and limitations

mlxtop is built for macOS on Apple Silicon and for Linux (x86_64 and
aarch64). Windows and Intel Macs aren’t supported targets for this release.

On Linux, memory and swap come from `/proc/meminfo`, paging rates from
`/proc/vmstat`, pressure level from the `MemAvailable` ratio blended with
`/proc/pressure/memory` stalls, GPU readings from `nvidia-smi` when present,
and thermals from `/sys/class/thermal`. Counters without a source are shown
as unavailable. Install the prebuilt static binary with the
[terminal installer](#install-from-terminal), download it from the
[latest release](https://github.com/maximpri/mlxtop/releases/latest), or
build from source with `cargo install --path . --locked`.

| Runtime | Available information |
| --- | --- |
| oMLX | Models, processes, prompt and response speed, requests, cache activity, and extra memory counters when available |
| llama.cpp / llama-server | Active slots and summed output counts; average rates and active/deferred queue counts when `/metrics` is enabled. Optional usage file adds full prompt history alongside native polling. |
| KoboldCpp | Last reported input/output counts and rates through `/api/extra/perf` |
| Ollama | Loaded models, resident VRAM and context capacity from `/api/ps`; completed counts and decode speed through the usage recorder |
| LM Studio / llmster | Loaded instances and context capacity from native APIs, with older API fallback; completed counts, speed and first-token timing through the usage recorder |
| vLLM, SGLang | Prometheus active/waiting queues, sampled server token rates, cache statistics, KV occupancy and cumulative mean first-token timing |
| mlx-serve | Prometheus active/waiting queues, live generation rate (running requests included), prefill rate and progress while a prompt is being processed, prefix-token cache reuse and MLX memory; the model and each running request's prompt, cached and output counts from `/metrics.json`. Start the server with `--metrics`. |
| MLX-LM, LocalAI, Jan, GPT4All | Process detection and available-model catalogue; completed request counts and explicitly supplied timing through the usage recorder |

Provider URLs, bearer authentication, default ports and client setup are covered
in the [provider guide](docs/USER_GUIDE.md#provider-endpoints).
The dashboard monitors one selected server at a time. Native model catalogues
cannot provide live generation speed; completed usage requires client integration.

Overview leads with host-critical resources: memory (pressure, composition of
wired/app/compressed/cache/free RAM and the GPU wired limit), compression
(macOS) and paging, then prompt load, then a compact throughput row. SYSINFO
holds model/state and hardware details; every chart has one border and an
independent plot. SWAP usage is a horizontal capacity bar. Numeric axes
fit visible measurements in their actual units; only percentages use 0–100.
Every chart is graded green/yellow/red by its thresholds: memory by OS
pressure, GPU, paging, compression and cache by configured bands, and
generation and prefill against their rolling baseline. Captured colors are
kept as history scrolls. Resident occupancy
includes reclaimable file cache and does not establish a warning by itself.
**Prompt load means prompt size in input tokens, including cached tokens.**
The headline gives the selected request's exact size; each bar represents one
observed request and shows its own compact size label, such as `12.0k` for
12,000 tokens. The panel also shows the change from the previous observed
request, freshness, and cached/uncached segments when reported. Selected prompts
also show output counts and request-specific decode speed: `LIVE`, a completed
request's `AVG`, or the retained `LAST` sample. Queue charts complement the system
metrics; OS process-memory details live in Top. First-token latency appears only when
explicitly measured client timings are supplied. See the
[operator charts](docs/USER_GUIDE.md#operator-charts) for scales and data sources.
Prompt counts also appear in the static report. Journal records each
observed request with its prompt count and request-specific cached count when
available. Polling is sampled: requests that finish between polls can be missed.
See [request telemetry setup](docs/USER_GUIDE.md#request-token-telemetry) for
provider selection, custom ports and response-only integrations. The optional
[Python client helper](scripts/record_usage.py) extracts counters from completed
responses and appends them to the usage file without storing response content.

To test unpublished RCs between this checkout and an oMLX server, use
`python3 scripts/rc.py push SSH_HOST`, then `python3 scripts/rc.py run SSH_HOST`.
Use `fetch SSH_HOST` to download the staged RC back to a compatible Mac.
See [private RC testing](docs/USER_GUIDE.md#private-rc-testing-over-ssh).

The oMLX connection defaults to `127.0.0.1:8080` and reads settings from
`~/.config/omlx-coding/server.env`. If you’re missing live readings, check the
[setup guide](docs/USER_GUIDE.md#omlx-telemetry).

The available readings depend on your runtime. Missing measurements are shown
as unavailable. GPU activity reflects the whole system, not individual models.
mlxtop never estimates response speed from CPU or GPU use. When live oMLX data
is unavailable, it may use recent completion logs and show how old those readings
are.

The dashboard can help you spot a slowdown and the conditions around it, but
it can’t prove what caused it. Charts, prompt history and the Journal cover
the current session; restarting mlxtop begins a new history.

## Privacy and local access

mlxtop runs without sudo and has no analytics. It reads system counters,
process information, and supported provider APIs and logs. Your project files
and model settings stay untouched, and it doesn’t send inference requests.

Diagnostic logs are saved locally at `~/Library/Logs/mlxtop/mlxtop.log` on
macOS and `~/.local/state/mlxtop/mlxtop.log` on Linux (following
`XDG_STATE_HOME` when set). They
include counters and model or provider names, but exclude prompts, model output,
request bodies, and API keys. These logs aren’t uploaded. By default, provider
credentials are sent only to endpoints on your own machine.

See the [data sources](docs/USER_GUIDE.md#data-sources-and-privacy),
[crash diagnostics](docs/USER_GUIDE.md#crash-diagnostics), and
[security policy](SECURITY.md) for details.

## Help and contributions

The [user guide](docs/USER_GUIDE.md) covers setup, controls, troubleshooting,
and the readings in each view. The [roadmap](https://github.com/maximpri/mlxtop/blob/main/OPEN_SOURCE_ROADMAP.md) describes
planned work.

If something breaks, [open an issue](https://github.com/maximpri/mlxtop/issues)
with your mlxtop version (`mlxtop --version`), macOS version, Mac model, runtime,
and steps to reproduce it. Remove private data from screenshots and logs.
Use [SECURITY.md](SECURITY.md) to report a vulnerability.

Documentation fixes, bug reports, and provider adapters are welcome. If a setup
step tripped you up, improving it is a useful place to start. Read
[CONTRIBUTING.md](CONTRIBUTING.md) for development checks and design guidelines.
CI enforces at least 90% production Rust line coverage on macOS and Linux.
See the [coverage workflow](CONTRIBUTING.md#coverage-gate) to reproduce the
reports locally.
For larger changes, open an issue first so we can discuss the approach.

## Acknowledgments

mlxtop was developed with Duet coding agent.

## License

[MIT](LICENSE). Dependencies keep their [own licenses](THIRD_PARTY_NOTICES.md).
Contributions use the same MIT terms. mlxtop is an independent project and
isn’t an Apple product.
