# mlxtop user guide

[Back to the README](../README.md) · [Quick start](../README.md#try-it)

Detailed controls, runtime setup, and explanations of the dashboard readings.

## What it shows

| View | Question answered | Key information |
| --- | --- | --- |
| Overview | Is inference healthy, and what limits it now? | Generation/prefill rate, diagnosis, cache/queue, memory, paging/compression and GPU |
| MLX Top | Which process owns the workload? | PID, command, model, CPU, memory %, RSS, page-ins and serving state |
| Journal | What changed during this session? | Request lifecycle, provider/model changes, paging, pressure, compression, GPU, thermal and recovery events |

Overview uses a balanced six-chart layout: generation, prefill and cache on the
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
them. Missing individual counters are shown as `—`, while a wholly unavailable
allocator group is summarized as `allocator counters not exposed`; RSS, GPU
load and model memory are never substituted for it.

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
~/Library/Logs/mlxtop/mlxtop.log
```

Follow it while reproducing a crash:

```sh
tail -f ~/Library/Logs/mlxtop/mlxtop.log
```

Set `MLXTOP_LOG_PATH` to use another path. The log records session lifecycle,
sampling duration, provider/API availability, sampler failures, terminal
errors and panic backtraces. A normal shutdown ends with `event=process_exit`;
if that record is missing, the process was interrupted or crashed. The active
log is capped at 8 MiB and rotated once to `mlxtop.log.1`.

It contains counters and model/provider names for diagnosis, but never prompts,
model output, request bodies or provider API keys. Diagnostics are local-only
and are not uploaded.

## Data sources and privacy

mlxtop reads macOS counters from `sysctl`, `memory_pressure`, `vm_stat`,
`ioreg`, `pmset` and `ps`. It reads local provider endpoints and logs only for
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
[OPEN_SOURCE_ROADMAP.md](../OPEN_SOURCE_ROADMAP.md).

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
