#!/bin/sh
# SPDX-License-Identifier: MIT
# Install the published macOS (Apple Silicon) or Linux (x86_64, aarch64)
# binary without requiring Rust or sudo.
set -eu

main() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64)
            target=aarch64-apple-darwin
            extension=dmg
            platform='Apple Silicon'
            tools='diskutil pkgutil'
            ;;
        Linux/x86_64 | Linux/amd64)
            target=x86_64-unknown-linux-musl
            extension=tar.gz
            platform='Linux x86_64'
            tools='tar'
            ;;
        Linux/aarch64 | Linux/arm64)
            target=aarch64-unknown-linux-musl
            extension=tar.gz
            platform='Linux aarch64'
            tools='tar'
            ;;
        *)
            printf '%s\n' 'mlxtop requires macOS on Apple Silicon or Linux on x86_64 or aarch64.' >&2
            exit 1
            ;;
    esac

    for command in curl grep awk mktemp $tools; do
        command -v "$command" >/dev/null 2>&1 || {
            printf 'Required command not found: %s\n' "$command" >&2
            exit 1
        }
    done
    if [ "$extension" = dmg ]; then
        # macOS 27 deprecates hdiutil attach/detach in favor of diskutil image;
        # older releases only have hdiutil.
        if diskutil image --help >/dev/null 2>&1; then
            attach_dmg() { diskutil image attach --readOnly --nobrowse --mountPoint "$2" "$1" >/dev/null; }
            detach_dmg() { diskutil eject "$1" >/dev/null; }
        elif command -v hdiutil >/dev/null 2>&1; then
            attach_dmg() { hdiutil attach "$1" -readonly -nobrowse -mountpoint "$2" >/dev/null; }
            detach_dmg() { hdiutil detach "$1" >/dev/null; }
        else
            printf '%s\n' 'Required command not found: hdiutil' >&2
            exit 1
        fi
    fi
    if command -v sha256sum >/dev/null 2>&1; then
        checksum='sha256sum'
    elif command -v shasum >/dev/null 2>&1; then
        checksum='shasum -a 256'
    else
        printf '%s\n' 'Required command not found: sha256sum or shasum' >&2
        exit 1
    fi

    repository=https://github.com/maximpri/mlxtop
    version="${MLXTOP_VERSION:-}"
    if [ -z "$version" ]; then
        # GitHub redirects the latest release to its tag; prereleases are
        # never the latest release.
        latest=$(curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --head --output /dev/null --write-out '%{url_effective}' \
            "$repository/releases/latest")
        version="${latest##*/tag/}"
    fi
    version="${version#v}"
    printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$' || {
        printf 'Could not determine an mlxtop release version (got: %s).\n' "$version" >&2
        exit 1
    }
    name="mlxtop-${version}-${target}"
    archive="${name}.${extension}"
    base="$repository/releases/download/v${version}"
    install_dir="${MLXTOP_INSTALL_DIR:-$HOME/.local/bin}"
    data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/mlxtop/${version}"
    case "$install_dir" in
        /*) ;;
        *) printf '%s\n' 'MLXTOP_INSTALL_DIR must be an absolute path.' >&2; exit 1 ;;
    esac
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mlxtop-install.XXXXXX")
    staged_binary=''
    mounted=0
    mount_dir="$work_dir/mount"
    cleanup() {
        if [ "$mounted" = 1 ]; then
            detach_dmg "$mount_dir" || return
        fi
        rm -rf "$work_dir"
        if [ -n "$staged_binary" ]; then rm -f "$staged_binary"; fi
    }
    trap cleanup EXIT
    trap 'exit 1' HUP INT TERM

    printf 'Downloading mlxtop %s for %s...\n' "$version" "$platform"
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
        "$base/$archive" -o "$work_dir/$archive"
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
        "$base/SHA256SUMS" -o "$work_dir/SHA256SUMS"
    # SHA256SUMS lists every platform's download; verify this one only.
    awk -v file="$archive" '$2 == file || $2 == "*" file' \
        "$work_dir/SHA256SUMS" > "$work_dir/SHA256SUMS.selected"
    [ -s "$work_dir/SHA256SUMS.selected" ] || {
        printf 'No checksum published for %s.\n' "$archive" >&2
        exit 1
    }
    (cd "$work_dir" && $checksum -c SHA256SUMS.selected)

    if [ "$extension" = dmg ]; then
        mkdir "$mount_dir"
        attach_dmg "$work_dir/$archive" "$mount_dir"
        mounted=1
        pkgutil --expand-full "$mount_dir/Install mlxtop.pkg" "$work_dir/expanded"
        detach_dmg "$mount_dir"
        mounted=0
        package="$work_dir/expanded/mlxtop-component.pkg/Payload/usr/local"
        binary="$package/bin/mlxtop"
        notices="$package/share/mlxtop/$version"
    else
        tar -xzf "$work_dir/$archive" -C "$work_dir"
        binary="$work_dir/$name/mlxtop"
        notices="$work_dir/$name"
    fi
    "$binary" --version

    mkdir -p "$install_dir" "$data_dir"
    cp "$notices/LICENSE" "$notices/THIRD_PARTY_NOTICES.md" "$data_dir/"
    cp -R "$notices/licenses" "$data_dir/"
    # Rename within the destination filesystem so a failed download or copy
    # cannot replace an existing installation with a partial binary.
    staged_binary=$(mktemp "$install_dir/.mlxtop.XXXXXX")
    cp "$binary" "$staged_binary"
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
