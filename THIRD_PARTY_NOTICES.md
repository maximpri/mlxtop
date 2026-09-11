# Third-party software notices

mlxtop's first-party source is licensed under MIT. It also links Rust crates
that remain under their respective licenses.

## Direct dependencies

| Package | Current locked version | Declared license |
| --- | --- | --- |
| crossterm | 0.28.1 | MIT |
| ratatui | 0.29.0 | MIT |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |

The complete dependency graph and exact versions are recorded in `Cargo.lock`.
CI evaluates every resolved dependency with `cargo-deny`. The current graph is
limited to the following accepted SPDX licenses and exceptions:

- MIT
- Apache-2.0
- Apache-2.0 WITH LLVM-exception
- BSL-1.0
- Unicode-3.0
- Unlicense
- Zlib

No dependency is relicensed as MIT by this project. Source packages downloaded
by Cargo contain their original copyright and license files. Anyone distributing
a compiled binary must also preserve and distribute the license texts and
notices required by the dependencies included in that binary; this summary does
not replace those texts.

The policy is enforced by `deny.toml`. Re-run `cargo deny check licenses` after
changing dependencies and review both `Cargo.lock` and the upstream license
files before publishing a release.
