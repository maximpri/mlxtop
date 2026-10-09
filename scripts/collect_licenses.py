#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Collect the license texts of every dependency built into a release.

Writes OUTPUT/<name>-<version>/<license files> and OUTPUT/DEPENDENCIES.json,
covering the crates built for all release targets (build dependencies
included). License files are found anywhere in a crate's source, so nested
notices such as ring's third-party licenses are kept.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
)
PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT")
SKIP_DIRS = {".git", "target"}


def metadata(manifest, target):
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--manifest-path", str(manifest), "--filter-platform", target],
        text=True,
    )
    return json.loads(output)


def release_packages(manifest):
    """Packages reachable from the root for any release target."""
    packages = {}
    for target in TARGETS:
        data = metadata(manifest, target)
        by_id = {package["id"]: package for package in data["packages"]}
        nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
        root = data["resolve"]["root"]
        pending, seen = [root], set()
        while pending:
            node_id = pending.pop()
            if node_id in seen:
                continue
            seen.add(node_id)
            for dependency in nodes[node_id]["deps"]:
                kinds = {kind["kind"] for kind in dependency["dep_kinds"]}
                if kinds - {"dev"}:
                    pending.append(dependency["pkg"])
        for node_id in seen - {root}:
            package = by_id[node_id]
            packages[(package["name"], package["version"])] = package
    return [packages[key] for key in sorted(packages)]


def license_files(root):
    found = []
    for directory, subdirectories, files in os.walk(root):
        subdirectories[:] = sorted(d for d in subdirectories if d not in SKIP_DIRS)
        for name in files:
            if name.upper().startswith(PREFIXES):
                found.append(Path(directory, name).relative_to(root).as_posix())
    return sorted(found)


def collect(manifest, output):
    output.mkdir(parents=True, exist_ok=False)
    # Sources of other targets' dependencies may not be downloaded yet.
    subprocess.run(["cargo", "fetch", "--locked", "--manifest-path", str(manifest)],
                   check=True, stdout=subprocess.DEVNULL)
    entries = []
    for package in release_packages(manifest):
        source = Path(package["manifest_path"]).parent
        files = license_files(source)
        if not files:
            raise SystemExit("no license file found for {} {}".format(package["name"], package["version"]))
        destination = output / "{}-{}".format(package["name"], package["version"])
        for relative in files:
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / relative, target)
        entries.append({
            "name": package["name"],
            "version": package["version"],
            "license": package["license"],
            "files": files,
        })
    (output / "DEPENDENCIES.json").write_text(json.dumps(entries, indent=2) + "\n")
    return entries


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="New directory to write the license bundle to")
    parser.add_argument("--manifest-path", type=Path,
                        default=Path(__file__).resolve().parent.parent / "Cargo.toml")
    args = parser.parse_args(argv)
    entries = collect(args.manifest_path, args.output)
    print("Collected licenses for {} dependencies into {}".format(len(entries), args.output))


if __name__ == "__main__":
    sys.exit(main())
