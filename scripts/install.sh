#!/bin/sh
# SPDX-License-Identifier: MIT
# Install the published Apple Silicon binary without requiring Rust or sudo.
set -eu

main() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) ;;
        *) printf '%s\n' 'mlxtop requires macOS on Apple Silicon.' >&2; exit 1 ;;
    esac

    for command in curl shasum tar mktemp; do
        command -v "$command" >/dev/null 2>&1 || {
            printf 'Required command not found: %s\n' "$command" >&2
            exit 1
        }
    done

    version=1.0.0
    archive="mlxtop-${version}-aarch64-apple-darwin.tar.gz"
    base="https://github.com/maximpri/mlxtop/releases/download/v${version}"
    install_dir="${MLXTOP_INSTALL_DIR:-$HOME/.local/bin}"
    data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/mlxtop/${version}"
    case "$install_dir" in
        /*) ;;
        *) printf '%s\n' 'MLXTOP_INSTALL_DIR must be an absolute path.' >&2; exit 1 ;;
    esac
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mlxtop-install.XXXXXX")
    staged_binary=''
    trap 'rm -rf "$work_dir"; if [ -n "$staged_binary" ]; then rm -f "$staged_binary"; fi' EXIT
    trap 'exit 1' HUP INT TERM

    printf 'Downloading mlxtop %s for Apple Silicon...\n' "$version"
    curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
        "$base/$archive" -o "$work_dir/$archive"
    curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
        "$base/SHA256SUMS" -o "$work_dir/SHA256SUMS"
    (cd "$work_dir" && shasum -a 256 -c SHA256SUMS)
    tar -xzf "$work_dir/$archive" -C "$work_dir"
    package="$work_dir/mlxtop-${version}-aarch64-apple-darwin"
    "$package/mlxtop" --version

    mkdir -p "$install_dir" "$data_dir"
    cp "$package/LICENSE" "$package/THIRD_PARTY_NOTICES.md" "$data_dir/"
    cp -R "$package/licenses" "$data_dir/"
    # Rename within the destination filesystem so a failed download or copy
    # cannot replace an existing installation with a partial binary.
    staged_binary=$(mktemp "$install_dir/.mlxtop.XXXXXX")
    cp "$package/mlxtop" "$staged_binary"
    chmod 755 "$staged_binary"
    mv -f "$staged_binary" "$install_dir/mlxtop"
    staged_binary=''

    printf '\nInstalled %s/mlxtop\n' "$install_dir"
    case ":$PATH:" in
        *":$install_dir:"*) printf '%s\n' 'Run: mlxtop' ;;
        *) printf 'Run: "%s/mlxtop"\nAdd "%s" to your PATH to run it as mlxtop.\n' \
            "$install_dir" "$install_dir" ;;
    esac
}

main "$@"
