# Making mlxtop the Best Open-Source Local LLM Monitor

Status: proposed roadmap

Research date: 2026-08-27

## Executive decision

mlxtop should own one problem better than any general system monitor:

> Given a local LLM request, show whether it is healthy, how fast it is serving,
> what is queued, and whether memory, cache, GPU, thermal, or provider behavior
> is limiting it.

That is a stronger position than becoming another general-purpose `top` clone.
The product should remain local-first and read-only by default, combine provider
telemetry with host evidence, and explain conclusions without inventing data.

The current application is a promising alpha with a distinctive UI and useful
macOS pressure analysis. It is not yet ready to be treated as a stable,
extensible open-source 1.0 platform. The primary blockers are architecture,
provider breadth, packaging hygiene, integration testing, and release/community
infrastructure.

## What is already strong

- The product has three non-redundant jobs: current health in Overview, live
  ownership in MLX Top, and meaningful transitions in Journal.
- Historical chart tones are stored with their samples and rendered through a
  deterministic width-aware trace, so later state does not rewrite history;
  color remains secondary to semantic status labels.
- Unknown values stay unavailable rather than being rendered as healthy zeroes.
- Telemetry has source and freshness information.
- Sampling is off the render/input thread.
- Severity uses recovery bands to reduce threshold flicker.
- The binary is native Rust, forbids unsafe code, and has a small runtime
  dependency set.
- CI, an MIT license, contributor guidance, static output, and binary-only
  deployment already exist.

These are product principles worth preserving as explicit compatibility rules.

## Current-state review

### Architecture

The application is concentrated in one 6,600-line `src/main.rs`. That file owns
domain types, native collection, subprocess management, rate calculations,
severity policy, process detection, oMLX HTTP and log parsing, history, journal
generation, every TUI view, input handling, static output, terminal lifecycle,
CLI parsing, and all tests.

This has several consequences:

- A provider contribution requires understanding unrelated UI and platform
  code.
- Policy and presentation are coupled through strings such as impact/state
  labels.
- The contributor guide asks for provider adapters, but there is no adapter
  trait or module boundary to implement.
- Platform expansion would add more conditionals to the same file.
- Unit tests can exercise parsers, but integration seams are difficult to
  substitute or fake.

### Collection and correctness

- macOS collection shells out sequentially to `sysctl`, `memory_pressure`,
  `vm_stat`, `ioreg`, `pmset`, and `ps`. Individual commands have timeouts, but
  a complete sample can still be delayed by several slow commands.
- The UI receives full cloned histories over an unbounded channel. This is
  acceptable at today's scale but is the wrong long-term backpressure model.
- Process-name detection recognizes multiple runtimes, but structured serving
  telemetry is currently oMLX-specific.
- The hand-written HTTP/1.1 client is intentionally small, but it is not a good
  foundation for multiple providers, TLS, redirects, chunked responses,
  connection reuse, proxy behavior, or richer authentication.
- Provider-level model identity is applied to the aggregate runtime view; the
  data model is not yet designed for multiple providers, endpoints, or models
  running concurrently.
- Completion-log modification time is only an approximation of observation
  time.
- There is no canonical capability model. An adapter cannot currently say
  “model inventory is available, TTFT is unavailable, and queue depth is stale.”

### UX

- Overview, MLX Top, and Journal have a good conceptual separation.
- Overview now leads with generation throughput and uses stepped time traces
  for GPU, memory and paging as supporting evidence. The user still cannot focus a
  chart, change its time window, inspect a point, or choose metrics.
- Medium terminals use a compact operational strip and responsive process
  columns. The hard minimum-size screen remains a warning rather than a useful
  narrow-mode dashboard.
- There is no persistent configuration, theme selection, ASCII/limited-color
  mode, mouse support, command palette, or discoverable in-app settings.
- MLX Top has per-process sorting and filtering but no drill-down view that
  correlates a selected process/model with its own request and resource
  history.
- Journal is session-only and has no export, search expression, bookmarks, or
  event detail.
- Static mode is human-readable only; there is no stable JSON/NDJSON contract.

### Testing and operations

- There are 45 useful unit tests, mostly for parsing, chart primitives, rates,
  thresholds, and small state rules.
- There are no provider fixture suites, fake-server integration tests, Ratatui
  snapshot tests, PTY keyboard tests, platform collector contract tests, fuzz
  targets, long-running sampler tests, or overhead benchmarks.
- CI checks formatting, Clippy, tests, release compilation, and ShellCheck, but
  does not check packaging, dependencies, MSRV, documentation links, release
  artifacts, or terminal snapshots.
- There is no persistent diagnostic log for failed collectors or providers.

### Open-source readiness

`cargo package --list --allow-dirty` currently includes
`install_mlx_lm_server_macos.sh`, `omlx-watch`, and the private deployment
helper. Those files are operationally related to one local setup, not the
minimal mlxtop crate. Publishing the crate in this state would ship unrelated
assets.

Cargo also reports that package `documentation`, `homepage`, and `repository`
metadata are missing. This checkout has no configured Git remote and no tags.
The repository has no changelog, security policy, code of conduct, issue forms,
pull-request template, support policy, release workflow, checksummed binaries,
SBOM, or build provenance.

If `1.0.0` has not been published, use a pre-1.0 version until the public data
model and provider contracts stabilize. If it has been published, keep SemVer
monotonic and define the compatibility promise before the next release.

## What comparable projects teach us

| Project | Primary strength | Lesson for mlxtop |
| --- | --- | --- |
| [htop](https://github.com/htop-dev/htop) | Configurable, searchable interactive process viewer | Keep navigation conventional and make meters/columns configurable. |
| [btop](https://github.com/aristocratos/btop) | Polished graphs, themes, presets, mouse support, process details | Add adaptive layouts, focus mode, themes, and selectable graph symbols without obscuring data. |
| [bottom](https://github.com/ClementTsang/bottom) | Cross-platform Rust monitor with configurable widget layouts | Separate collectors from widgets and make layouts/configuration data-driven. |
| [nvtop](https://github.com/Syllo/nvtop) | Multi-vendor accelerator process monitoring | Treat accelerator support as a capability matrix, not a single GPU percentage. |
| [nvitop](https://github.com/XuehaiPan/nvitop) | Direct NVML access, async collection, process drill-down, exporter/library APIs | Prefer direct APIs, sparse polling and caches; provide a read-only mode and reusable core. |
| [macmon](https://github.com/vladkens/macmon) | Apple Silicon TUI plus Rust library, JSON, and Prometheus output | Reuse or integrate a maintained native collector instead of expanding command parsing; make mlxtop's core usable without the TUI. |
| [Glances](https://github.com/nicolargo/glances) | Plugin architecture, remote/API modes, and many export formats | Normalize metrics once, then let TUI, JSON, Prometheus, and remote consumers share them. |
| [tokentop](https://github.com/tokentopapp/tokentop) | Multi-provider plugins, auto-discovery, demo mode, responsive layouts, themes | Make provider onboarding easy and the product demonstrable without a configured runtime. Do not duplicate its cloud cost/session niche. |
| [vllmstat](https://github.com/bryanvine/vllmstat) | Dense inference-first telemetry without a Grafana stack | Keep concurrency, throughput and latency dominant; hide irrelevant panels and degrade unavailable values explicitly. |
| [InfraWhisperer/llmtop](https://github.com/InfraWhisperer/llmtop) | Runtime-aware queue, KV-cache and latency views across inference servers | Preserve focused detail/model/device views and make provider capabilities explicit instead of forcing one universal screen. |

Licenses differ. In particular, some GPU-monitor TUI code is GPL-licensed.
Borrow interaction principles and data-model ideas, not implementation code,
without a deliberate license review.

## Native provider telemetry to support

Provider-native metrics are more trustworthy than inferring serving speed from
CPU/GPU activity. The first adapter set should use the following official
interfaces:

| Runtime | Useful official interface | Initial mlxtop support |
| --- | --- | --- |
| oMLX | Existing health/admin API and completion logs | Preserve current support, move it behind the common adapter contract, and add fixtures. |
| Ollama | [`/api/ps`](https://docs.ollama.com/api/ps) for loaded model/VRAM/context and [response usage fields](https://docs.ollama.com/api/usage) for prompt/decode counts and durations | Inventory from `/api/ps`; only show throughput when observed from real response telemetry or logs. Never send a synthetic request just to measure it. |
| llama.cpp | Optional [Prometheus-compatible `/metrics`](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) | Parse prompt/decode throughput, token counters, busy/deferred requests, and context high-water marks when metrics are enabled. |
| vLLM | [Production `/metrics`](https://docs.vllm.ai/en/latest/usage/metrics/) and [per-request metrics](https://docs.vllm.ai/en/latest/features/per_request_metrics/) | Support running/waiting requests, KV-cache use, TTFT, inter-token latency, queue time, prefill/decode time, throughput, preemptions, and errors. |
| Hugging Face TGI | [Prometheus `/metrics`](https://huggingface.co/docs/text-generation-inference/en/reference/metrics) | Support queue/batch size, request latency, input/output tokens, prefill/decode duration, and mean time per token. |
| LM Studio | [Native v1 REST API](https://lmstudio.ai/docs/developer/rest) | Start with server discovery, loaded-model inventory, load state, and context. Report serving metrics only when an official response/event exposes them. |

Later adapters can cover LocalAI, SGLang, MLX LM, KoboldCpp, and remote
OpenMetrics endpoints. A process signature alone should qualify only for
identity and host-resource correlation, never for request throughput or cache
claims.

## Canonical observability model

Every adapter should map into one provider-neutral model. Align names and units
where practical with the evolving
[OpenTelemetry GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai),
while isolating that mapping because the conventions are still evolving.

### Identity

- Provider and adapter version
- Endpoint and transport security
- Model identifier, family, quantization, context capacity, and instance ID
- Associated PIDs and accelerator devices
- Adapter capabilities

### Serving load

- Active, waiting, deferred, and failed requests
- Batch size and scheduler state
- Prompt, generated, cached, and reasoning-token counters when available
- Request and token rates computed from monotonic counters

### Latency and throughput

- Time to first token (TTFT), preferably p50/p95/p99
- Time per output token or inter-token latency (TPOT/ITL)
- End-to-end and queue latency
- Prefill and generation throughput
- Load time and model cold-start state

### Capacity and efficiency

- Model memory and accelerator memory
- KV-cache occupancy, evictions, and preemptions
- Prefix-cache hit rate and cached-token ratio
- Context high-water mark and context limit
- Power and thermal state when supported

### Host correlation

- Native memory pressure and available memory
- Swap-in/out and compression/decompression rates
- CPU, accelerator, memory-bandwidth, temperature, and power data
- Per-process RSS/CPU and provider/model ownership

### Provenance contract

Every optional measurement should carry:

```text
value | unit | source | observed_at | freshness | confidence | error
```

Recommended source tiers:

1. **Native provider metric** — direct live API/OpenMetrics value.
2. **Provider completion event** — measured result from a real completed request.
3. **Provider log** — parsed event with explicit timestamp and known format.
4. **Host observation** — process/system fact, not a serving-speed estimate.

The UI must never silently promote a lower tier or stale value to live native
telemetry.

## Target architecture

Turn the crate into a library with a thin binary:

```text
src/
  lib.rs
  bin/mlxtop.rs
  config.rs
  domain/
    metric.rs
    model.rs
    sample.rs
    event.rs
    severity.rs
  sampling/
    engine.rs
    history.rs
    rates.rs
  platform/
    mod.rs
    macos.rs
    linux.rs
  providers/
    mod.rs
    discovery.rs
    omlx.rs
    ollama.rs
    llama_cpp.rs
    vllm.rs
    tgi.rs
    lm_studio.rs
  export/
    json.rs
    prometheus.rs
  ui/
    app.rs
    input.rs
    theme.rs
    widgets/
    views/
tests/
  fixtures/providers/
  provider_contract.rs
  tui_snapshots.rs
  cli.rs
```

Core contracts should look conceptually like this:

```rust
trait PlatformCollector {
    fn capabilities(&self) -> PlatformCapabilities;
    fn sample(&mut self) -> Result<SystemObservation, CollectError>;
}

trait ProviderAdapter {
    fn id(&self) -> ProviderId;
    fn discover(&self, context: &DiscoveryContext) -> Vec<EndpointCandidate>;
    fn capabilities(&self) -> ProviderCapabilities;
    fn poll(&mut self) -> Result<ProviderObservation, ProviderError>;
}
```

Use typed enums for provider, impact, event kind, health, units, and freshness.
Rendering strings belong in the UI. Thresholds and explanations belong in a
policy layer that consumes typed observations.

Additional architectural rules:

- Use a bounded latest-value channel for snapshots; the UI needs the newest
  state, not a backlog of cloned histories.
- Give fast host counters, slow hardware counters, provider APIs, and logs
  independent polling cadences and deadlines.
- Keep histories in the sampler/store and send immutable snapshots or deltas.
- Replace the hand-written HTTP client with a maintained client behind a small
  transport trait so fake servers can test authentication, timeouts, stale
  data, malformed payloads, and backoff.
- Keep authentication loopback-only by default and redact all secrets from
  errors and diagnostic logs.
- Do not use unstable in-process dynamic Rust plugins. Start with first-party
  adapter modules/crates; if third-party plugins become necessary, prefer a
  versioned external JSON protocol with an explicit permission manifest.

## Product design

### Overview: “Is inference healthy?”

Keep only the current conclusion and decisive evidence:

- Model/provider/state and telemetry freshness
- TTFT, generation tok/s, queue, active requests, errors
- Memory/KV/GPU headroom
- Bottleneck, confidence, evidence, and next action
- Three small histories selected from the most relevant current metrics

The chart set should adapt. When an LLM is serving, prioritize generation
throughput, queue depth, and memory/KV pressure. When no serving telemetry is
available, show host capacity and clearly label the reduced capability.

### MLX Top: “Who owns the work?”

Make rows provider/model instances rather than only OS processes. Suggested
columns:

```text
PROVIDER  MODEL  STATE  ACTIVE  QUEUE  GEN tok/s  TTFT p95  KV%  RSS/VRAM  PID
```

Selecting a row should open a drill-down screen with:

- Associated processes and endpoint
- Serving and host charts on the same time axis
- Cache/context details
- Recent model-specific journal events
- Capability and provenance detail

Process signaling can be added later, behind confirmation and a `--readonly`
default. It is not necessary for the first public release.

### Journal: “What changed and did it matter?”

Record transitions, not sample noise:

- Request burst started/ended
- Queue became non-zero or recovered
- TTFT/TPOT crossed a sustained threshold
- Cache pressure, eviction, or preemption
- Memory pressure/paging/thermal transition
- Provider disconnected, became stale, or recovered
- Model loaded/unloaded/switched
- Bottleneck classification changed, with evidence

Support text search, event-kind filters, model/provider filters, bookmarks, and
JSONL export. Persisting the journal should be opt-in and local, with bounded
retention.

### Terminal quality

Borrow mature TUI conventions:

- Auto/full/compact layouts rather than a hard minimum-size warning
- Focus/expand the selected widget
- 24-bit, 256-color, 16-color, monochrome, and ASCII graph modes
- Configurable theme and thresholds
- Vim and arrow-key navigation
- Mouse support as optional enhancement
- Command palette for discoverability
- `--demo --seed N` mode for screenshots, bug reports, and deterministic tests
- Respect `NO_COLOR`, terminal capabilities, and reduced-motion/low-refresh use

## Output and integration modes

The TUI should be one consumer of the core, not the only product surface.

Add:

```text
mlxtop                         interactive TUI
mlxtop --once                 human static report
mlxtop --json                 stable one-shot JSON
mlxtop watch --ndjson         streaming observations/events
mlxtop serve --listen 127.0.0.1:9098
mlxtop providers              discovery/capability diagnostics
mlxtop doctor                 platform and endpoint diagnostics
mlxtop demo --seed 42         deterministic synthetic workload
```

The local server should expose `/health`, `/snapshot`, and `/metrics` and bind
to loopback by default. Prometheus/OpenMetrics output makes mlxtop useful beside
the TUI and follows the successful pattern used by macmon, Glances, vLLM,
llama.cpp, and TGI.

## Prioritized implementation roadmap

### P0 — Make the repository safe to publish

Target: one focused, reproducible crate that strangers can understand and
build.

- Add `repository`, `homepage`, `documentation`, `rust-version`, and explicit
  `include` metadata to `Cargo.toml`.
- Remove oMLX installer/watch scripts from the crate package and mlxtop release
  payload. Move them to their owning project or a clearly labeled `contrib/`
  area excluded from packaging.
- Split `main.rs` into `lib`, domain, sampling, platform, providers, UI, and
  CLI modules without changing behavior.
- Introduce typed errors and internal diagnostic logging.
- Add `CHANGELOG.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `SUPPORT.md`, issue
  forms, and a pull-request template. GitHub's
  [healthy-contribution guidance](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions)
  describes the expected community files.
- Document supported macOS versions/chips, required permissions, data sources,
  network behavior, and privacy guarantees.

Exit criteria:

- `cargo package --list` contains only mlxtop assets.
- A new contributor can find collector/provider/UI boundaries in under five
  minutes.
- All existing behavior and tests pass after the module split.

### P1 — Establish trustworthy provider contracts

Target: one canonical model and four high-quality local adapters.

- Add capability, provenance, freshness, endpoint, model-instance, serving,
  and host-observation types.
- Move oMLX behind the adapter contract.
- Implement Ollama, llama.cpp, and LM Studio adapters.
- Add vLLM and TGI OpenMetrics adapters immediately after the local-first set;
  their metrics provide an excellent reference for the canonical schema.
- Add provider fixtures, fake HTTP servers, authentication/redaction tests,
  malformed-response tests, timeout/backoff tests, and stale-data tests.
- Show adapter health and missing capabilities in `mlxtop providers` and the
  help UI.

Exit criteria:

- At least four runtimes pass the same adapter contract suite.
- No provider can display a metric it did not explicitly observe.
- Multiple simultaneous endpoints/models remain distinct throughout the UI.

### P1 — Make the monitor diagnostically excellent

Target: answer “why is this request slow?” within ten seconds.

- Add TTFT, TPOT/ITL, queue, KV/cache, error, and throughput histories.
- Make Overview charts relevance-driven.
- Replace process-only rows with provider/model instance rows and add
  drill-down.
- Add sustained-window policy rules and display confidence/evidence.
- Correlate journal events on a shared monotonic timeline.
- Add adaptive layout, focus mode, terminal capability fallback, and demo mode.

Exit criteria:

- A fixture scenario can distinguish queue saturation, memory paging, thermal
  limiting, KV pressure, provider outage, and healthy GPU saturation.
- Every displayed diagnosis lists the measurements that caused it.
- The UI remains useful at 80x24, 120x40, and ultrawide sizes.

### P2 — Make the core reusable and observable

Target: TUI, automation, and dashboards share one data model.

- Add stable JSON, NDJSON, and Prometheus/OpenMetrics output.
- Publish the collector/domain API as a documented Rust library.
- Add opt-in local bounded journal persistence and export.
- Add `doctor` diagnostics and a sanitized support bundle.
- Evaluate a macmon library integration for richer sudoless Apple Silicon
  frequency, power, temperature, and bandwidth data.
- Add Linux host collection and NVIDIA/AMD accelerator backends only after the
  capability model is proven on macOS.

Exit criteria:

- TUI and exporters consume the same immutable snapshot types.
- Prometheus names/units are documented and stable within a release series.
- Running without the TUI is fully supported and tested.

### P2 — Build a serious quality pipeline

Target: contributors can change collectors and UI without regressions.

- Unit and property tests for rates, counter reset/wrap, hysteresis, freshness,
  and event suppression.
- Golden provider fixtures from sanitized real responses across supported
  versions.
- Ratatui `TestBackend` snapshots for compact, normal, wide, monochrome, and
  unavailable-data states.
- PTY tests for navigation, filtering, pause/reset, resize, and clean terminal
  restoration.
- Fuzz provider/log/native parsers.
- Soak tests for bounded channels, stale endpoints, and repeated process churn.
- Benchmarks for collection overhead, render latency, memory growth, and
  history retention.
- CI for stable and MSRV Rust, macOS and Linux compilation, `cargo package`,
  documentation, ShellCheck, dependency policy/audit, and release dry-runs.

Suggested performance budgets:

- Idle average CPU below 1% on a representative Apple Silicon Mac.
- Resident memory below 50 MiB with default history.
- Keyboard-to-frame latency below 100 ms while a provider times out.
- No unbounded queues or history growth.
- No more than one idle journal event per minute unless state changes.

### P3 — Make installation and contribution first-class

Target: a trustworthy release can be installed in one command.

- Automate tagged GitHub Releases, Apple Silicon and Intel macOS binaries,
  checksums, shell installer, and Homebrew formula. The Rust
  [`dist`](https://github.com/axodotdev/cargo-dist) project can generate and
  publish these artifacts.
- Publish to crates.io if the package name is available and `cargo install`
  provides a good experience.
- Generate shell completions and a man page.
- Attach SBOM and signed build provenance. GitHub supports
  [artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
  for binaries and SBOMs.
- Add automated dependency updates, vulnerability/policy checks, and pinned
  release actions.
- Add README screenshots, a deterministic demo recording, architecture docs,
  provider support matrix, troubleshooting, privacy/security model, and a
  “good first adapter” contributor tutorial.
- Use release notes and a public roadmap with small, independently mergeable
  issues.

Exit criteria:

- A clean Mac installs, verifies, runs demo mode, and uninstalls without a Rust
  toolchain.
- Release artifacts can be traced to a tagged source commit and verified.
- A first-time contributor can implement a fixture-backed adapter without
  editing the UI.

## Recommended issue breakdown

Create issues small enough to review independently:

1. Define `Observed<T>`, units, freshness, and capabilities.
2. Split the current collector into a macOS platform module.
3. Split oMLX into a provider module with fixtures.
4. Move policy/classification to typed domain code.
5. Split the TUI into app, theme, widgets, and three view modules.
6. Replace snapshot channel with bounded latest-value delivery.
7. Replace custom HTTP with a tested transport abstraction.
8. Add `mlxtop providers` and `mlxtop doctor`.
9. Add Ollama inventory and usage parsing.
10. Add llama.cpp OpenMetrics parsing.
11. Add LM Studio model inventory.
12. Add vLLM/TGI metric normalization.
13. Add JSON/NDJSON schema and CLI integration tests.
14. Add adaptive compact layout and TUI snapshots.
15. Add deterministic demo scenarios.
16. Clean crate/release contents and complete Cargo metadata.
17. Add community health and issue/PR templates.
18. Add tagged, attested release automation and Homebrew packaging.

## What not to build yet

- A cloud token-cost dashboard; tokentop already targets that problem.
- A replacement for btop/htop outside LLM-relevant host evidence.
- A proxy that must sit in every inference request path.
- Provider metrics inferred from GPU utilization.
- Arbitrary in-process plugins with an unstable Rust ABI.
- Process termination controls before identity mapping and confirmation are
  unquestionably safe.
- A web UI before the core schema and export contracts are stable.

## Definition of “best open source”

mlxtop earns that description when it is:

- **Trustworthy** — values have units, provenance, freshness, and honest gaps.
- **Diagnostic** — it explains queue, memory, cache, compute, and thermal
  bottlenecks with evidence.
- **Broad but coherent** — multiple runtimes map into one capability-aware
  model without lowest-common-denominator fiction.
- **Fast** — provider failures cannot freeze input or grow memory.
- **Composable** — TUI, JSON, Prometheus, and library users share the same core.
- **Installable** — signed, checksummed binaries and Homebrew installation do
  not require a Rust toolchain.
- **Contributable** — adapters are isolated, fixtures are easy to add, and
  maintainers document compatibility and review expectations.
- **Focused** — every screen and metric helps explain local LLM behavior.

The fastest route is not adding more panels. It is building a trustworthy
adapter and metric core, then letting a restrained TUI make that evidence easy
to understand.

## Research sources

Primary project and vendor sources used for this review:

- [htop](https://github.com/htop-dev/htop)
- [btop](https://github.com/aristocratos/btop)
- [bottom](https://github.com/ClementTsang/bottom)
- [nvtop](https://github.com/Syllo/nvtop)
- [nvitop](https://github.com/XuehaiPan/nvitop)
- [macmon](https://github.com/vladkens/macmon)
- [asitop](https://github.com/tlkh/asitop)
- [Glances](https://github.com/nicolargo/glances)
- [tokentop](https://github.com/tokentopapp/tokentop)
- [Ollama running-model API](https://docs.ollama.com/api/ps)
- [Ollama usage metrics](https://docs.ollama.com/api/usage)
- [llama.cpp server metrics](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- [vLLM production metrics](https://docs.vllm.ai/en/latest/usage/metrics/)
- [vLLM per-request metrics](https://docs.vllm.ai/en/latest/features/per_request_metrics/)
- [Hugging Face TGI metrics](https://huggingface.co/docs/text-generation-inference/en/reference/metrics)
- [LM Studio REST API](https://lmstudio.ai/docs/developer/rest)
- [OpenTelemetry GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai)
- [GitHub healthy-contribution guidance](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions)
- [GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [Rust dist](https://github.com/axodotdev/cargo-dist)
