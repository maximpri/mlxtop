#!/usr/bin/env bash
# SPDX-License-Identifier: MIT

# shellcheck disable=SC2029
# Build and deploy this repository to a remote machine over SSH, or deploy a
# release that GitHub-hosted runners built (--release TAG).
#
# A release is unpacked and built in a private staging directory first. The
# stable "current" symlink is changed only after the build and health check
# succeed, so a failed deployment leaves the previous release usable.

set -Eeuo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd -P)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
readonly SCRIPT_DIR PROJECT_ROOT
readonly -a DEPLOY_PATHS=(
  Cargo.toml
  Cargo.lock
  src
)

remote_host="${REMOTE_HOST:-${DEPLOY_HOST:-}}"
remote_user="${REMOTE_USER:-${DEPLOY_USER:-${USER:-}}}"
remote_port="${REMOTE_PORT:-22}"
remote_dir="${REMOTE_DIR:-${DEPLOY_DIR:-}}"
if [[ -z "$remote_dir" ]]; then
  remote_dir=\$HOME/mlxtop
fi
remote_tmp_dir="${REMOTE_TMP_DIR:-/tmp}"
ssh_key="${SSH_KEY:-${DEPLOY_SSH_KEY:-}}"
known_hosts_file="${SSH_KNOWN_HOSTS:-}"
strict_host_key_checking="${SSH_STRICT_HOST_KEY_CHECKING:-accept-new}"
keep_releases="${KEEP_RELEASES:-5}"
build_on_remote="${BUILD_ON_REMOTE:-1}"
release_tag="${RELEASE_TAG:-}"
release_repo="${RELEASE_REPO:-maximpri/mlxtop}"
restart_command="${RESTART_COMMAND:-}"
healthcheck_command="${HEALTHCHECK_COMMAND:-}"
dry_run=0

usage() {
  cat <<'EOF'
Usage: scripts/cicd.sh [deploy] [options]

Required environment:
  REMOTE_HOST                   Remote hostname or address

Environment:
  REMOTE_USER                   SSH user (defaults to the local user)
  REMOTE_PORT                   SSH port (default: 22)
  REMOTE_DIR                    Install root (default: $HOME/mlxtop)
  REMOTE_TMP_DIR                Remote staging parent (default: /tmp)
  SSH_KEY                       Private key passed to ssh and scp (otherwise
                                the normal SSH config/agent is used)
  SSH_KNOWN_HOSTS               known_hosts file for CI
  SSH_STRICT_HOST_KEY_CHECKING  SSH policy (default: accept-new)
  KEEP_RELEASES                 Number of releases to retain (default: 5)
  BUILD_ON_REMOTE               Build on target, 1 or 0 (default: 1)
  RELEASE_TAG                   Deploy this GitHub release instead of building
  RELEASE_REPO                  Repository for RELEASE_TAG (default: maximpri/mlxtop)
  RESTART_COMMAND               Optional command run from the current release
  HEALTHCHECK_COMMAND           Optional command run after restart

Examples:
  REMOTE_HOST=mac.example.com REMOTE_USER=deploy scripts/cicd.sh
  REMOTE_HOST=10.0.0.12 SSH_KEY="$HOME/.ssh/deploy" scripts/cicd.sh
  REMOTE_HOST=mac.example.com \
    RESTART_COMMAND='systemctl --user restart mlxtop' scripts/cicd.sh
  REMOTE_HOST=10.0.0.12 scripts/cicd.sh --release v2.1.1

Options:
  --host HOST                   Override REMOTE_HOST
  --user USER                   Override REMOTE_USER
  --port PORT                   Override REMOTE_PORT
  --remote-dir PATH             Override REMOTE_DIR
  --ssh-key PATH                Override SSH_KEY
  --keep-releases N             Override KEEP_RELEASES
  --skip-build                  Use target/release/mlxtop from this checkout
  --release TAG                 Deploy the GitHub-runner build of release TAG for
                                the remote platform, verified against SHA256SUMS
  --no-healthcheck              Skip the post-deploy health check
  --restart COMMAND             Override RESTART_COMMAND
  --healthcheck COMMAND         Override HEALTHCHECK_COMMAND
  --dry-run                     Validate and print settings without SSH
  -h, --help                    Show this help
EOF
}

die() {
  printf 'cicd.sh: %s\n' "$*" >&2
  exit 1
}

log() {
  printf 'cicd.sh: %s\n' "$*"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

reject_whitespace() {
  local name="$1"
  local value="$2"
  [[ "$value" != *[[:space:]]* ]] || die "$name must not contain whitespace"
}

# ssh concatenates its command arguments before handing them to the remote
# shell. Quote values that are intended to become positional parameters so
# paths and user-provided hook commands keep their original boundaries.
shell_quote() {
  local value="$1"
  value="$(printf '%s' "$value" | sed "s/'/'\\\\''/g")"
  printf "'%s'" "$value"
}

while (($#)); do
  case "$1" in
    deploy)
      shift
      ;;
    --host)
      (($# >= 2)) || die "--host requires a value"
      remote_host="$2"
      shift 2
      ;;
    --user)
      (($# >= 2)) || die "--user requires a value"
      remote_user="$2"
      shift 2
      ;;
    --port)
      (($# >= 2)) || die "--port requires a value"
      remote_port="$2"
      shift 2
      ;;
    --remote-dir)
      (($# >= 2)) || die "--remote-dir requires a value"
      remote_dir="$2"
      shift 2
      ;;
    --ssh-key)
      (($# >= 2)) || die "--ssh-key requires a value"
      ssh_key="$2"
      shift 2
      ;;
    --keep-releases)
      (($# >= 2)) || die "--keep-releases requires a value"
      keep_releases="$2"
      shift 2
      ;;
    --skip-build)
      build_on_remote=0
      shift
      ;;
    --release)
      (($# >= 2)) || die "--release requires a tag"
      release_tag="$2"
      shift 2
      ;;
    --no-healthcheck)
      healthcheck_command=:
      shift
      ;;
    --restart)
      (($# >= 2)) || die "--restart requires a value"
      restart_command="$2"
      shift 2
      ;;
    --healthcheck)
      (($# >= 2)) || die "--healthcheck requires a value"
      healthcheck_command="$2"
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1 (use --help for usage)"
      ;;
  esac
done

[[ -n "$remote_host" ]] || die "REMOTE_HOST is required"
[[ "$remote_port" =~ ^[0-9]+$ ]] || die "REMOTE_PORT must be a number"
((remote_port >= 1 && remote_port <= 65535)) || die "REMOTE_PORT is out of range"
[[ "$keep_releases" =~ ^[0-9]+$ ]] || die "KEEP_RELEASES must be a non-negative integer"
((keep_releases >= 1)) || die "KEEP_RELEASES must be at least 1"
[[ "$build_on_remote" == 0 || "$build_on_remote" == 1 ]] || die "BUILD_ON_REMOTE must be 0 or 1"

reject_whitespace REMOTE_HOST "$remote_host"
reject_whitespace REMOTE_USER "$remote_user"
reject_whitespace REMOTE_DIR "$remote_dir"
reject_whitespace REMOTE_TMP_DIR "$remote_tmp_dir"

for path in "${DEPLOY_PATHS[@]}"; do
  [[ -e "$PROJECT_ROOT/$path" ]] ||
    die "deploy path does not exist: $path"
done

if [[ -n "$release_tag" ]]; then
  [[ "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$ ]] ||
    die "--release expects a tag such as v2.1.1 or v2.2.0-rc.1"
  reject_whitespace RELEASE_REPO "$release_repo"
  # A release deploys a prebuilt binary; never build on the remote.
  build_on_remote=0
elif ((build_on_remote == 0)); then
  [[ -x "$PROJECT_ROOT/target/release/mlxtop" ]] ||
    die "--skip-build requires target/release/mlxtop; run cargo build --release first"
fi

require_command ssh
require_command scp
require_command tar
if [[ -n "$ssh_key" ]]; then
  [[ -f "$ssh_key" ]] || die "SSH_KEY does not exist: $ssh_key"
fi

git_sha="$(git -C "$PROJECT_ROOT" rev-parse --short=12 HEAD 2>/dev/null || true)"
[[ -n "$git_sha" ]] || git_sha=nogit
release_id="release-$(date -u +%Y%m%d%H%M%S)-${git_sha}-${RANDOM}${RANDOM}"
remote_target="$remote_host"
if [[ -n "$remote_user" ]]; then
  remote_target="${remote_user}@${remote_host}"
fi

ssh_args=(-T -p "$remote_port" -o BatchMode=yes)
scp_args=(-P "$remote_port")
if [[ -n "$ssh_key" ]]; then
  ssh_args+=(-i "$ssh_key")
  scp_args+=(-i "$ssh_key")
fi
if [[ -n "$strict_host_key_checking" ]]; then
  ssh_args+=(-o "StrictHostKeyChecking=$strict_host_key_checking")
  scp_args+=(-o "StrictHostKeyChecking=$strict_host_key_checking")
fi
if [[ -n "$known_hosts_file" ]]; then
  ssh_args+=(-o "UserKnownHostsFile=$known_hosts_file")
  scp_args+=(-o "UserKnownHostsFile=$known_hosts_file")
fi

log "release: $release_id"
log "target: $remote_target"
log "remote root: $remote_dir"
log "build on remote: $build_on_remote"
if [[ -n "$release_tag" ]]; then
  log "release: $release_repo $release_tag (built on GitHub runners)"
fi
if ((dry_run)); then
  log "dry run: no files were transferred"
  exit 0
fi

archive="$(mktemp "${TMPDIR:-/tmp}/mlxtop-deploy.XXXXXX.tar.gz")"
remote_prepare_output="$(mktemp "${TMPDIR:-/tmp}/mlxtop-remote.XXXXXX")"
remote_upload_dir=""
download_dir=""
cleanup() {
  rm -f "$archive"
  if [[ -n "$download_dir" ]]; then
    rm -rf "$download_dir"
  fi
  if [[ -n "$remote_prepare_output" ]]; then
    rm -f "$remote_prepare_output"
  fi
  if [[ -n "$remote_upload_dir" ]]; then
    cleanup_command="bash -s -- $(shell_quote "$remote_upload_dir")"
    ssh "${ssh_args[@]}" "$remote_target" "$cleanup_command" <<'REMOTE_CLEANUP' >/dev/null 2>&1 || true
set -Eeuo pipefail
rm -rf "$1"
REMOTE_CLEANUP
  fi
}
trap cleanup EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# Fetch the GitHub-runner build for the remote's platform and check it against
# the release's SHA256SUMS before anything is transferred.
fetch_release_binary() {
  local platform target version asset expected actual base
  platform="$(ssh "${ssh_args[@]}" "$remote_target" 'uname -sm')"
  case "$platform" in
    "Darwin arm64") target=aarch64-apple-darwin ;;
    "Linux x86_64") target=x86_64-unknown-linux-musl ;;
    "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl ;;
    *) die "no release build for remote platform: $platform" ;;
  esac
  version="${release_tag#v}"
  asset="mlxtop-$version-$target.tar.gz"
  download_dir="$(mktemp -d "${TMPDIR:-/tmp}/mlxtop-release.XXXXXX")"
  log "downloading $asset from $release_repo $release_tag"
  if command -v gh >/dev/null 2>&1; then
    # gh also reads draft releases, which are not public yet.
    gh release download "$release_tag" --repo "$release_repo" --dir "$download_dir" \
      --pattern "$asset" --pattern SHA256SUMS
  else
    require_command curl
    base="https://github.com/$release_repo/releases/download/$release_tag"
    curl -fsSL -o "$download_dir/$asset" "$base/$asset"
    curl -fsSL -o "$download_dir/SHA256SUMS" "$base/SHA256SUMS"
  fi
  expected="$(awk -v name="$asset" '$2 == name {print $1}' "$download_dir/SHA256SUMS")"
  [[ -n "$expected" ]] || die "SHA256SUMS does not list $asset"
  actual="$(sha256_of "$download_dir/$asset")"
  [[ "$actual" == "$expected" ]] || die "checksum mismatch for $asset"
  tar -xzf "$download_dir/$asset" -C "$download_dir"
  release_binary="$download_dir/mlxtop-$version-$target/mlxtop"
  [[ -x "$release_binary" ]] || die "release archive has no mlxtop binary"
  log "verified $asset ($target)"
}

if [[ -n "$release_tag" ]]; then
  require_command awk
  log "checking SSH connectivity"
  ssh "${ssh_args[@]}" "$remote_target" true
  fetch_release_binary
  tar -czf "$archive" -C "$(dirname "$release_binary")" mlxtop
elif ((build_on_remote)); then
  archive_paths=()
  for path in "${DEPLOY_PATHS[@]}"; do
    [[ -e "$PROJECT_ROOT/$path" ]] && archive_paths+=("$path")
  done
  tar -czf "$archive" -C "$PROJECT_ROOT" "${archive_paths[@]}"
else
  # A local-build deployment needs only the already-built executable. Keep
  # source files and development helpers out of the remote release.
  tar -czf "$archive" -C "$PROJECT_ROOT/target/release" mlxtop
fi

log "checking SSH connectivity"
ssh "${ssh_args[@]}" "$remote_target" true

log "creating remote staging directory"
prepare_command="bash -s -- $(shell_quote "$remote_tmp_dir") $(shell_quote "$release_id")"
ssh "${ssh_args[@]}" "$remote_target" "$prepare_command" >"$remote_prepare_output" <<'REMOTE_PREPARE'
set -Eeuo pipefail
tmp_root="$1"
release_id="$2"
case "$tmp_root" in
  '~') tmp_root="$HOME" ;;
  '~/'*) tmp_root="$HOME/${tmp_root:2}" ;;
  '$HOME') tmp_root="$HOME" ;;
  '$HOME/'*) tmp_root="$HOME/${tmp_root:6}" ;;
esac
mkdir -p "$tmp_root"
upload_dir="$(mktemp -d "$tmp_root/mlxtop-${release_id}.XXXXXX")"
printf '%s\n' "$upload_dir"
REMOTE_PREPARE
remote_upload_dir="$(cat "$remote_prepare_output")"
rm -f "$remote_prepare_output"
remote_prepare_output=""
[[ -n "$remote_upload_dir" ]] || die "remote staging directory was not returned"
reject_whitespace REMOTE_UPLOAD_DIR "$remote_upload_dir"

log "uploading deploy archive"
scp "${scp_args[@]}" "$archive" "$remote_target:$remote_upload_dir/archive.tar.gz"

log "installing release"
install_command="bash -s -- $(shell_quote "$remote_dir") $(shell_quote "$release_id") \
  $(shell_quote "$remote_upload_dir") $(shell_quote "$keep_releases") \
  $(shell_quote "$build_on_remote") $(shell_quote "$healthcheck_command") \
  $(shell_quote "$restart_command")"
ssh "${ssh_args[@]}" "$remote_target" "$install_command" <<'REMOTE_INSTALL'
set -Eeuo pipefail

base="$1"
release_id="$2"
upload_dir="$3"
keep_releases="$4"
build_on_remote="$5"
healthcheck_command="$6"
restart_command="$7"

case "$base" in
  '~') base="$HOME" ;;
  '~/'*) base="$HOME/${base:2}" ;;
  '$HOME') base="$HOME" ;;
  '$HOME/'*) base="$HOME/${base:6}" ;;
esac

[[ "$base" != *[[:space:]]* ]] || { printf 'remote root contains whitespace\n' >&2; exit 1; }
[[ "$release_id" =~ ^release-[0-9]{14}-[0-9A-Za-z_-]+$ ]] || { printf 'invalid release id\n' >&2; exit 1; }
mkdir -p "$base/releases"
if [[ -e "$base/current" && ! -L "$base/current" ]]; then
  printf 'refusing to replace non-symlink: %s/current\n' "$base" >&2
  exit 1
fi

stage_dir="$(mktemp -d "$base/.stage-${release_id}.XXXXXX")"
release_dir="$base/releases/$release_id"
cleanup() {
  rm -rf "$upload_dir"
  if [[ -n "${stage_dir:-}" && -d "$stage_dir" ]]; then rm -rf "$stage_dir"; fi
}
trap cleanup EXIT
[[ ! -e "$release_dir" ]] || { printf 'release already exists\n' >&2; exit 1; }

tar -xzf "$upload_dir/archive.tar.gz" -C "$stage_dir"
if ((build_on_remote)); then
  command -v cargo >/dev/null 2>&1 || { printf 'cargo is required on remote host\n' >&2; exit 1; }
  (cd "$stage_dir" && cargo build --release --locked)
fi

if ((build_on_remote)); then
  binary="$stage_dir/target/release/mlxtop"
else
  binary="$stage_dir/mlxtop"
fi
[[ -x "$binary" ]] || { printf 'release binary not found: %s\n' "$binary" >&2; exit 1; }
"$binary" --help >/dev/null
chmod 755 "$binary"
if ((build_on_remote)); then
  printf '%s\n' "$release_id" > "$stage_dir/.release"
fi
mv "$stage_dir" "$release_dir"
stage_dir=""

# Replace the active symlink. macOS mv follows a symlink to a directory when
# it is used as the destination, so remove the old link before moving in the
# new one; never replace a real directory.
next_link="$base/.current-$release_id"
rm -f "$next_link"
ln -s "$release_dir" "$next_link"
if [[ -e "$base/current" || -L "$base/current" ]]; then
  [[ -L "$base/current" ]] || { printf 'refusing to replace non-symlink: %s/current\n' "$base" >&2; exit 1; }
  rm -f "$base/current"
fi
mv "$next_link" "$base/current"

cd "$base/current"
if [[ -n "$restart_command" ]]; then bash -c "$restart_command"; fi
if [[ -n "$healthcheck_command" ]]; then
  bash -c "$healthcheck_command"
else
  if ((build_on_remote)); then
    "$base/current/target/release/mlxtop" --help >/dev/null
  else
    "$base/current/mlxtop" --help >/dev/null
  fi
fi

# Keep only the newest matching releases. Always retain the active release.
release_list="$({
  for candidate in "$base/releases"/release-*; do
    if [[ -d "$candidate" && ! -L "$candidate" ]]; then
      candidate_name="${candidate##*/}"
      [[ "$candidate_name" =~ ^release-[0-9]{14}-[0-9A-Za-z_-]+$ ]] && printf '%s\n' "$candidate"
    fi
  done
} | LC_ALL=C sort -r)"
release_number=0
while IFS= read -r candidate; do
  [[ -n "$candidate" ]] || continue
  release_number=$((release_number + 1))
  if ((release_number > keep_releases)) && [[ "$candidate" != "$release_dir" ]]; then
    rm -rf "$candidate"
  fi
done <<< "$release_list"

printf 'deployed %s\n' "$release_id"
printf 'current %s\n' "$base/current"
REMOTE_INSTALL

log "deployment complete"
