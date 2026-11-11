#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build, transfer and run private native RCs over SSH, without GitHub."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tarfile
import tempfile
import time

PROJECT = Path(__file__).resolve().parent.parent
OUTPUT = PROJECT / "target" / "private-rc"
REMOTE_ROOT = '.local/share/mlxtop/rc'
RC_VERSION = r"[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*"

# The same small installer runs locally and on the remote host (Python 3.8+).
# It verifies the complete payload before atomically promoting the RC symlink.
INSTALLER = r'''
import hashlib, io, json, os, platform, re, shutil, subprocess, sys, tarfile, tempfile
from pathlib import Path

def install(payload, expected, root):
    if hashlib.sha256(payload).hexdigest() != expected:
        raise ValueError("RC checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
        members = archive.getmembers()
        names = [member.name for member in members]
        allowed = {"mlxtop", "build.json", "LICENSE", "THIRD_PARTY_NOTICES.md"}
        if set(names) != allowed or len(names) != len(allowed) or not all(member.isfile() for member in members):
            raise ValueError("Invalid RC archive contents")
        metadata = json.load(archive.extractfile("build.json"))
        version = metadata["version"]
        build_id = metadata["build_id"]
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*", version):
            raise ValueError("Expected an RC version")
        if not re.fullmatch(re.escape(version) + r"-[A-Za-z0-9.-]+", build_id):
            raise ValueError("Invalid RC build ID")
        machine = platform.machine().lower()
        machine = {"aarch64": "arm64", "amd64": "x86_64"}.get(machine, machine)
        native = platform.system() + "-" + machine
        if metadata["platform"] != native:
            raise ValueError("RC is for " + metadata["platform"] + "; this host is " + native + ". Build on a matching host.")
        root = Path(root).expanduser().resolve()
        root.mkdir(parents=True, exist_ok=True)
        releases = root / "releases"
        releases.mkdir(exist_ok=True)
        destination = releases / build_id
        current = root / "current"
        if current.exists() and not current.is_symlink():
            raise ValueError("Refusing to replace a real current directory")
        if destination.exists():
            if (destination / "rc.tar.gz").read_bytes() != payload:
                raise ValueError("A different payload already uses RC build ID " + build_id)
            link = root / (".current-" + build_id)
            link.symlink_to(destination)
            os.replace(link, current)
            return destination
        stage = Path(tempfile.mkdtemp(prefix=".stage-", dir=root))
        link = stage / "next"
        try:
            for member in members:
                (stage / member.name).write_bytes(archive.extractfile(member).read())
            binary = stage / "mlxtop"
            binary.chmod(0o755)
            actual = subprocess.check_output([str(binary), "--version"], text=True, timeout=10).strip()
            if actual != "mlxtop " + version:
                raise ValueError("Binary version does not match RC manifest")
            subprocess.run([str(binary), "--help"], check=True, stdout=subprocess.DEVNULL, timeout=10)
            (stage / "rc.tar.gz").write_bytes(payload)
            (stage / "SHA256SUMS").write_text(expected + "  rc.tar.gz\n")
            stage.rename(destination)
            link = root / (".current-" + build_id)
            link.symlink_to(destination)
            os.replace(link, current)
        finally:
            if stage.exists():
                shutil.rmtree(stage)
            if link.is_symlink():
                link.unlink()
        return destination
'''


def platform_id():
    import platform
    machine = platform.machine().lower()
    machine = {"aarch64": "arm64", "amd64": "x86_64"}.get(machine, machine)
    return platform.system() + "-" + machine


def install(payload, checksum, root):
    namespace = {}
    exec(INSTALLER, namespace)
    return namespace["install"](payload, checksum, root)


def build():
    manifest = (PROJECT / "Cargo.toml").read_text()
    version = re.search(r'^version\s*=\s*"([^"]+)"', manifest, re.MULTILINE).group(1)
    if not re.fullmatch(RC_VERSION, version):
        raise ValueError("Set an X.Y.Z-rc.N version in Cargo.toml and Cargo.lock first")
    build_env = os.environ.copy()
    if sys.platform == "darwin":
        # Match the active Xcode toolchain even when a newer standalone SDK exists.
        build_env["SDKROOT"] = subprocess.check_output(["xcrun", "--no-cache", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
    subprocess.run(["cargo", "build", "--release", "--locked", "--target-dir", str(OUTPUT / "build")], cwd=PROJECT, env=build_env, check=True)
    binary = OUTPUT / "build" / "release" / "mlxtop"
    revision = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=PROJECT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=PROJECT))
    build_id = "{}-{}{}-{}".format(version, revision, ".dirty" if dirty else "", time.time_ns())
    metadata = {"version": version, "build_id": build_id, "revision": revision, "dirty": dirty, "platform": platform_id()}
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        archive.add(binary, arcname="mlxtop", recursive=False)
        for name in ["LICENSE", "THIRD_PARTY_NOTICES.md"]:
            archive.add(PROJECT / name, arcname=name, recursive=False)
        contents = (json.dumps(metadata, indent=2) + "\n").encode()
        info = tarfile.TarInfo("build.json")
        info.size = len(contents)
        archive.addfile(info, io.BytesIO(contents))
    payload = buffer.getvalue()
    checksum = hashlib.sha256(payload).hexdigest()
    destination = install(payload, checksum, OUTPUT)
    print("Built {}\nArchive: {}\nSHA256: {}".format(build_id, destination / "rc.tar.gz", checksum), flush=True)
    return payload, checksum


def ssh_command(args, code, interactive=False):
    if not args.host or args.host.startswith("-") or any(c.isspace() for c in args.host):
        raise ValueError("Use an SSH host alias or user@host")
    command = ["ssh", "-t" if interactive else "-T"]
    if args.port:
        command += ["-p", str(args.port)]
    if args.identity:
        command += ["-i", args.identity]
    return command + [args.host, code]


def remote_python(code):
    return "python3 -c " + shlex.quote(code)


def push(args):
    if args.dry_run:
        print("Build native {} RC; transfer via SSH to {}; verify and stage at ~/{}/current/mlxtop".format(platform_id(), args.host, REMOTE_ROOT))
        return
    payload, checksum = build()
    code = INSTALLER + "\npayload = sys.stdin.buffer.read()\nprint(install(payload, " + repr(checksum) + ", Path.home() / " + repr(REMOTE_ROOT) + "))\n"
    subprocess.run(ssh_command(args, remote_python(code)), input=payload, check=True)
    print("RC staged. Test interactively: python3 scripts/rc.py run " + shlex.quote(args.host))


def fetch(args):
    code = """from pathlib import Path
import sys
root = Path.home() / %r / 'current'
sys.stdout.buffer.write((root / 'SHA256SUMS').read_bytes().split()[0] + b'\\n')
sys.stdout.buffer.write((root / 'rc.tar.gz').read_bytes())
""" % REMOTE_ROOT
    if args.dry_run:
        print("Download RC from {} via SSH; verify and stage at {}".format(args.host, OUTPUT / "current"))
        return
    with tempfile.TemporaryFile() as output:
        subprocess.run(ssh_command(args, remote_python(code)), stdout=output, check=True)
        output.seek(0)
        checksum = output.readline(66).decode().strip()
        if not re.fullmatch(r"[a-f0-9]{64}", checksum):
            raise ValueError("Remote did not return a valid RC checksum")
        payload = output.read()
    destination = install(payload, checksum, OUTPUT)
    print("Downloaded and verified {}\nRun: python3 scripts/rc.py run".format(destination))


def run(args):
    flags = (["--once"] if args.once else []) + (["--record", args.record] if args.record else [])
    if args.host:
        code = 'exec "$HOME/' + REMOTE_ROOT + '/current/mlxtop"' + "".join(" " + shlex.quote(flag) for flag in flags)
        command = ssh_command(args, code, interactive=not args.once)
    else:
        binary = OUTPUT / "current" / "mlxtop"
        if not binary.is_file():
            raise ValueError("No local RC staged; use build or fetch first")
        command = [str(binary)] + flags
    if args.dry_run:
        print(shlex.join(command))
        return
    subprocess.run(command, check=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build", help="Build and stage this checkout as a native RC")
    build_parser.add_argument("--stage-for-fetch", action="store_true", help="Also stage under ~/.local/share/mlxtop/rc so another machine can fetch it")
    for name, help_text in [("push", "Build here and stage on a matching remote host"), ("fetch", "Download a staged remote RC to this machine"), ("run", "Run the staged RC locally or over SSH")]:
        command = commands.add_parser(name, help=help_text)
        command.add_argument("host", nargs="?" if name == "run" else None, help="SSH alias or user@host")
        command.add_argument("--port", type=int)
        command.add_argument("--identity", help="SSH private key path")
        command.add_argument("--dry-run", action="store_true", help="Print the operation without building or connecting")
        if name == "run":
            command.add_argument("--once", action="store_true", help="Print a static report instead of opening the dashboard")
            command.add_argument("--record", metavar="FILE", help="Append every sample to FILE on the machine that runs mlxtop (replay with: mlxtop replay FILE)")
    args = parser.parse_args(argv)
    if getattr(args, "port", None) is not None and not 1 <= args.port <= 65535:
        parser.error("--port must be between 1 and 65535")
    try:
        if args.command == "build":
            payload, checksum = build()
            if args.stage_for_fetch:
                destination = install(payload, checksum, Path.home() / REMOTE_ROOT)
                print("Ready for remote fetch: " + str(destination))
        else:
            {"push": push, "fetch": fetch, "run": run}[args.command](args)
    except (ValueError, OSError, subprocess.SubprocessError, tarfile.TarError) as error:
        print("rc: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
