#!/usr/bin/env bash
# SPDX-License-Identifier: MIT

# Compatibility launcher. The monitor itself is a native Rust/Ratatui binary.
set -euo pipefail
ROOT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"

if [[ -x "$ROOT_DIR/target/release/mlxtop" ]]; then
  exec "$ROOT_DIR/target/release/mlxtop" "$@"
fi
if command -v cargo >/dev/null 2>&1; then
  exec cargo run --quiet --manifest-path "$ROOT_DIR/Cargo.toml" -- "$@"
fi
printf 'mlxtop is not built. Install Rust (cargo), or build target/release/mlxtop.\n' >&2
exit 1
