# Contributing

Thank you for helping improve mlxtop. Bug reports, documentation, provider
fixtures and focused implementation changes are all useful contributions.

## Before opening a change

Run the local checks:

~~~sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
~~~

For UI changes, test both the interactive dashboard and the static report:

~~~sh
./target/release/mlxtop
./target/release/mlxtop --once
~~~

## Design boundaries

- Keep Overview focused on current health and LLM impact.
- Keep MLX Top focused on live process/resource inspection.
- Keep Journal focused on meaningful historical transitions.
- Never display historical provider values as live telemetry.
- Show telemetry provenance and age whenever a provider API is unavailable.
- Use fixed-width stepped time-series traces for indicator history. One
  displayed column maps to one captured sample; new samples enter on the right
  and old samples leave on the left once the viewport is full. Gaps must remain
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
  terminal must retain status, throughput, diagnosis and action; process-table
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
