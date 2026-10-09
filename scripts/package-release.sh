#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Package a built mlxtop binary as a release archive:
#   mlxtop-VERSION-TARGET/{mlxtop,BUILD-INFO.json,docs,licenses,scripts,...}
# The payload directory is left next to the archive for scripts/package-dmg.sh.
set -euo pipefail

usage() {
    printf 'Usage: %s TARGET BINARY OUTPUT_DIRECTORY [LICENSES_DIRECTORY]\n' "$0" >&2
    exit 1
}
[[ $# == 3 || $# == 4 ]] || usage
target=$1
binary=$2
mkdir -p "$3"
output="$(cd -- "$3" && pwd -P)"
root="$(cd -- "$(dirname -- "$0")/.." && pwd -P)"

[[ -x "$binary" ]] || { printf 'Binary is not executable: %s\n' "$binary" >&2; exit 1; }
version="$("$binary" --version)"
version="${version#mlxtop }"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$ ]] || {
    printf 'Unsupported version: %s\n' "$version" >&2
    exit 1
}
commit="${GITHUB_SHA:-$(git -C "$root" rev-parse HEAD)}"
name="mlxtop-$version-$target"
stage="$output/$name"
rm -rf "$stage" "$output/$name.tar.gz"
mkdir -p "$stage/scripts"

cp "$binary" "$stage/mlxtop"
chmod 755 "$stage/mlxtop"
for file in CHANGELOG.md LICENSE README.md THIRD_PARTY_NOTICES.md; do
    cp "$root/$file" "$stage/$file"
done
cp "$root/scripts/record_usage.py" "$stage/scripts/record_usage.py"
# Tracked documentation only, exactly as committed.
while IFS= read -r path; do
    mkdir -p "$stage/$(dirname "$path")"
    cp "$root/$path" "$stage/$path"
done < <(git -C "$root" ls-files docs)

if [[ $# == 4 ]]; then
    cp -R "$4" "$stage/licenses"
else
    python3 "$root/scripts/collect_licenses.py" "$stage/licenses" >/dev/null
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha="$(sha256sum "$stage/mlxtop" | awk '{print $1}')"
else
    sha="$(shasum -a 256 "$stage/mlxtop" | awk '{print $1}')"
fi
cat > "$stage/BUILD-INFO.json" <<JSON
{
  "version": "$version",
  "target": "$target",
  "source_commit": "$commit",
  "binary_sha256": "$sha"
}
JSON
# No extended attributes or AppleDouble files in the archive.
COPYFILE_DISABLE=1 tar -C "$output" -czf "$output/$name.tar.gz" "$name"
printf '%s\n' "$output/$name.tar.gz"
