# Submitting mlxtop to homebrew-core

`mlxtop.rb` is ready to submit to [Homebrew/homebrew-core](https://github.com/Homebrew/homebrew-core).
It is the [maximpri/tap](https://github.com/maximpri/homebrew-tap) formula without the
tap's `bottle` block (Homebrew's CI builds bottles) and post-install caveats. The tap
version passes `brew test-bot` on macOS and Linux.

## Eligibility

Homebrew's [package acceptance policy](https://docs.brew.sh/Package-Acceptance-Policy)
requires a repository at least 30 days old (mlxtop: from 2026-10-11) and at least
30 forks, 30 watchers or 75 stars. A submission by the repository owner needs 90
forks, 90 watchers or 225 stars, so a user other than the maintainer may submit it
first.

## Steps

1. Fork and clone `Homebrew/homebrew-core`, and branch from the latest `main`.
2. Copy `mlxtop.rb` to `Formula/m/mlxtop.rb`. For a newer release, update `url` and
   `sha256` (`curl -sL <tarball url> | shasum -a 256`).
3. Check it locally:

   ```sh
   HOMEBREW_NO_INSTALL_FROM_SOURCE=1 brew install --build-from-source mlxtop
   brew test mlxtop
   brew audit --strict --new --online mlxtop
   brew style --fix --formula mlxtop
   ```

4. Commit as `mlxtop 2.1.1 (new formula)` and open a pull request. Complete the
   template, including whether AI assisted.

Once it merges, `brew install mlxtop` works without a tap. Then remove the tap
formula, or replace it with a `tap_migrations.json` entry pointing to
`homebrew/core`, so the two names do not conflict.
