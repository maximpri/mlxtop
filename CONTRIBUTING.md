# Contributing

Thank you for helping improve mlxtop. Bug reports, documentation, provider
fixtures and focused implementation changes are all useful contributions.

## Before opening a change

Run the local checks:

macOS builds require Xcode Command Line Tools (including the macOS SDK and
libclang) for the safe `libproc` wrapper's generated bindings. The application
continues to forbid unsafe Rust in its own source.

~~~sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 -m unittest discover -s scripts -p 'test_*.py'
shellcheck scripts/cicd.sh scripts/coverage.sh scripts/install.sh scripts/package-dmg.sh scripts/package-release.sh
cargo build --release --locked
~~~

### Coverage gate

Production line coverage must stay at or above 90 percent on each supported
platform. Install the tools once, then run the gate:

~~~sh
cargo install cargo-llvm-cov --locked
rustup component add llvm-tools-preview
scripts/coverage.sh
~~~

Cargo, rustc and the LLVM coverage tools must use matching toolchains. If
multiple Rust installations are present, select the intended toolchain before
running the gate. `LLVM_COV` and `LLVM_PROFDATA` can point to matching tools.

The script writes `coverage.json`, `summary.txt` and `uncovered.txt` to
`target/coverage/` (override with `COVERAGE_DIR`) and fails below
`COVERAGE_MIN_LINES` (default `90`). Tests live only in `src/tests/`; each
module declares `#[cfg(test)] #[path = "tests/<module>.rs"] mod tests;`. The
report excludes exactly `(^|/)src/tests/[^/]+\.rs$`, so the denominator is all
production Rust compiled for the host platform. The script refuses to run if a
production file contains `cfg(coverage)`, `coverage(off)`, `cfg(not(test))`,
or any other `#[cfg(test)]` item. Code that reads the host goes through the
`host::Host` seam, so collectors are tested with recorded command and file
fixtures instead of being excluded.

After running the gate, generate a browsable report from the same profiles:

~~~sh
cargo llvm-cov report --html --output-dir target/coverage \
  --ignore-filename-regex '(^|/)src/tests/[^/]+\.rs$'
~~~

Open `target/coverage/html/index.html`. CI uploads the JSON, summary and
uncovered-line reports as `coverage-macos-latest` and `coverage-ubuntu-latest`
artifacts. Coverage describes code exercised by tests; review assertions and
missing cases alongside the percentage.

For UI changes, test both the interactive dashboard and the static report:

~~~sh
./target/release/mlxtop
./target/release/mlxtop --once
~~~

## Release candidates

Use SemVer release candidates before a final release: `1.1.2-rc.1`,
`1.1.2-rc.2`, then `1.1.2`. Increment the positive RC number for each new
candidate of the same target version. When starting a new target version,
restart at `rc.1`.

Update the package version in `Cargo.toml` and the `mlxtop` entry in
`Cargo.lock` together, and add the candidate's changes to `CHANGELOG.md`.
The CLI and dashboard obtain their version from Cargo. Rebuild with
`cargo build --locked` and check `./target/debug/mlxtop --version`.

Build and test candidates on your own machine: `python3 scripts/rc.py push
HOST` builds a native RC locally, stages it on a matching remote test host
(for example an Apple Silicon Mac running the runtime under test) and leaves
stable installs untouched; `python3 scripts/rc.py run HOST` opens it there.
See [Private RC testing over SSH](docs/USER_GUIDE.md#private-rc-testing-over-ssh).
Candidates get no Git tag and no GitHub release; only final versions are
tagged and built on GitHub-hosted runners. The README links and
`scripts/install.sh` follow GitHub's latest release, so they need no
per-release edits.

## Publishing a release

GitHub-hosted runners build every final release asset; release candidates
stay local (see above).

1. Update the version in `Cargo.toml` and `Cargo.lock`, add a
   `## X.Y.Z — YYYY-MM-DD` section to `CHANGELOG.md`, and merge the change
   through a pull request.
2. Tag the merge commit on `main` and push the tag:

   ~~~sh
   git tag -a vX.Y.Z -m "mlxtop X.Y.Z" && git push origin vX.Y.Z
   ~~~

3. The `Release` workflow tests and builds on native runners: the Apple
   Silicon macOS archive and DMG installer, and static musl archives for Linux
   x86_64 and ARM64. `scripts/collect_licenses.py` bundles the dependency
   license texts once for all targets, `scripts/package-release.sh` assembles
   each archive with `BUILD-INFO.json`, and one `SHA256SUMS` covers all four
   downloads. The workflow creates a **draft** release whose notes start from
   the version's changelog section. Only `vX.Y.Z` tags trigger it; RC tags
   such as `vX.Y.Z-rc.N` do not.
4. Review the draft's notes and assets, then publish it:

   ~~~sh
   gh release edit vX.Y.Z --draft=false --latest
   ~~~

Pull requests that change the workflow or packaging scripts run the same
builds without publishing; the assets are attached to the run as artifacts.

Deploy a published or draft release to a test host with
`scripts/cicd.sh --release vX.Y.Z --host HOST`. It picks the archive for the
host's platform and verifies it against `SHA256SUMS` before installing.

### Homebrew

The [maximpri/tap](https://github.com/maximpri/homebrew-tap) formula installs
with `brew install maximpri/tap/mlxtop`. After a release, open a version bump
on the tap, let its CI build bottles, then publish them:

~~~sh
brew bump-formula-pr --url https://github.com/maximpri/mlxtop/archive/refs/tags/vX.Y.Z.tar.gz maximpri/tap/mlxtop
gh workflow run publish.yml --repo maximpri/homebrew-tap -f pull_request=PR_NUMBER
~~~

`packaging/homebrew-core/` holds the formula and steps for submitting to
homebrew-core.

## Source layout

`main.rs` only declares modules and enters the application. `cli` handles arguments
and startup; `app` owns navigation, selection and alarms; `ui` and the dashboard
modules render terminal views. `terminal` owns restoration and the event loop.

`domain` contains shared observations. `collector` combines platform counters and
provider telemetry; `sampler` runs it off the UI thread. `history`, `request_history`
and `operator_history` own storage separately from rendering. `analysis` and
`diagnosis` interpret measured conditions; they do not render widgets.

`host`, `platform`, `processes`, `gpu`, and `process_memory` own system inputs.
`omlx`, `providers`, `provider_native`, and `transport` own API collection.
`runtime_diagnostics` carries sanitized connection observations and capability
information. `report` supplies the shared doctor/panel text and the static report.
`config`, `logging`, `formatting`, `parsing`, `json`, and `theme` provide scoped support.

Use explicit imports and crate-local interfaces. Keep core collection independent
of terminal widgets, provider errors free of response bodies/credentials, and all
test-only fixtures under `src/tests/` so coverage includes all production code.

## Design boundaries

Follow the [UX design rules](docs/UX_DESIGN.md) for naming, typography, color,
layout, metric semantics and interaction. Chart titles use lowercase words
with acronyms preserved: `prompt load`, `generation`, `GPU`.
The [chart specification](docs/CHART_SPEC.md) is mandatory for every chart:
use consistent green/yellow/red bands where defined, preserve captured colors,
and automatically scale numeric axes in their actual units. Only percentages
use 0–100. Consolidate related readings instead of duplicating summary cards.

- Keep Overview focused on current health and LLM impact.
- Keep MLX Top focused on live process/resource inspection.
- Keep Journal focused on meaningful historical transitions.
- Never display historical provider values as live telemetry.
- Show telemetry provenance and age whenever a provider API is unavailable.
- Use fixed-width stepped time-series traces for indicator history. One
  displayed column maps to one captured sample. Overview shares a trailing
  window and zoom across time series, widening samples for larger panels
  without decimation. New samples enter on the right. Gaps must remain
  disconnected, and a sample's recorded severity tone must not be recolored by
  a later refresh. If smoothing is needed for readability, make it causal and
  derive it from the current drawable resolution using only the samples up to
  that point. Keep raw readings and captured tones unchanged; never recalculate
  history from a future or centered display-time window.
- Treat color as a secondary cue. Every visible severity or operating condition
  must also have a semantic label that does not depend on color perception.
- Keep unavailable counters as unavailable; do not substitute a healthy zero.
- Preserve visual hierarchy: serving throughput is the outcome; GPU, memory,
  paging and compression are supporting evidence.
- Use responsive disclosure instead of squeezing cards or columns. A medium
  terminal must retain status, throughput, prompt sizes and resource readings; process-table
  columns may collapse in documented priority order.
- Keep keyboard hints contextual to the active view and reserve color for
  identity, severity and selected state rather than decoration.
- Keep provider network/authentication and metric parsing out of the native
  collector. Add or extend a provider adapter instead; process signatures may
  be used only for lightweight detection and labeling.

## Pull requests

Describe the user-visible behavior, the platform assumptions, and the checks
you ran. Include a terminal screenshot for substantial layout changes when
possible.

Do not include API keys, prompts, private logs or other workload data in an
issue, fixture or screenshot. Report suspected vulnerabilities according to
[SECURITY.md](SECURITY.md).

## Contribution license

By submitting a contribution, you confirm that you have the right to provide
it and agree that it is licensed under the project's MIT License.
