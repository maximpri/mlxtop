#!/usr/bin/env python3
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import collect_licenses


def touch(root, relative, text="license"):
    path = Path(root, relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


class LicenseFileTests(unittest.TestCase):
    def test_finds_nested_notices_and_ignores_other_files(self):
        with tempfile.TemporaryDirectory() as root:
            for relative in ["LICENSE-MIT", "COPYRIGHT", "src/README.md", "src/lib.rs",
                             "third_party/fiat/LICENSE", "Licence.txt", "target/LICENSE"]:
                touch(root, relative)
            self.assertEqual(
                collect_licenses.license_files(root),
                ["COPYRIGHT", "LICENSE-MIT", "Licence.txt", "third_party/fiat/LICENSE"],
            )


def fake_metadata(source):
    """One root depending on a library (normal), a build tool (build) and a test helper (dev)."""
    def package(name):
        return {"id": name, "name": name, "version": "1.0.0", "license": "MIT",
                "manifest_path": str(Path(source, name, "Cargo.toml"))}
    names = ["root", "library", "build-tool", "test-helper"]
    return {
        "packages": [package(name) for name in names],
        "resolve": {
            "root": "root",
            "nodes": [
                {"id": "root", "deps": [
                    {"pkg": "library", "dep_kinds": [{"kind": None}]},
                    {"pkg": "build-tool", "dep_kinds": [{"kind": "build"}]},
                    {"pkg": "test-helper", "dep_kinds": [{"kind": "dev"}]},
                ]},
                {"id": "library", "deps": []},
                {"id": "build-tool", "deps": []},
                {"id": "test-helper", "deps": []},
            ],
        },
    }


class CollectTests(unittest.TestCase):
    def test_bundles_runtime_and_build_dependencies_but_not_dev_only_ones(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as work:
            for name in ["library", "build-tool", "test-helper"]:
                touch(source, name + "/LICENSE", name)
            output = Path(work, "licenses")
            with mock.patch.object(collect_licenses, "metadata", return_value=fake_metadata(source)), \
                    mock.patch.object(collect_licenses.subprocess, "run"):
                entries = collect_licenses.collect(Path(source, "Cargo.toml"), output)
            self.assertEqual([entry["name"] for entry in entries], ["build-tool", "library"])
            self.assertEqual(Path(output, "library-1.0.0/LICENSE").read_text(), "library")
            self.assertTrue(Path(output, "DEPENDENCIES.json").is_file())

    def test_a_dependency_without_a_license_file_fails(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as work:
            touch(source, "library/LICENSE")
            with mock.patch.object(collect_licenses, "metadata", return_value=fake_metadata(source)), \
                    mock.patch.object(collect_licenses.subprocess, "run"):
                with self.assertRaises(SystemExit):
                    collect_licenses.collect(Path(source, "Cargo.toml"), Path(work, "licenses"))


if __name__ == "__main__":
    unittest.main()
