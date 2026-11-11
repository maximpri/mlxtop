# SPDX-License-Identifier: MIT
import argparse
import hashlib
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

import rc


def package(build="one", platform=None, binary_version="1.2.1-rc.1", extra=None):
    metadata = {"version": "1.2.1-rc.1", "build_id": "1.2.1-rc.1-" + build, "platform": platform or rc.platform_id()}
    files = {
        "build.json": json.dumps(metadata).encode(),
        "mlxtop": ("#!/bin/sh\nprintf 'mlxtop %s\\n' '" + binary_version + "'\n").encode(),
        "LICENSE": b"test license",
        "THIRD_PARTY_NOTICES.md": b"test notices",
    }
    if extra:
        files[extra] = b"unexpected"
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, data in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
    payload = output.getvalue()
    return payload, hashlib.sha256(payload).hexdigest()


class PrivateRcTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mlxtop-rc-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_verified_candidate_is_separate_and_repeated_download_is_safe(self):
        stable = self.root / "mlxtop"
        stable.write_text("stable installation")
        payload, checksum = package()
        candidate = rc.install(payload, checksum, self.root / "rc")
        self.assertEqual((self.root / "rc/current").resolve(), candidate)
        self.assertEqual(subprocess.check_output([str(candidate / "mlxtop"), "--version"], text=True).strip(), "mlxtop 1.2.1-rc.1")
        self.assertEqual(rc.install(payload, checksum, self.root / "rc"), candidate)
        self.assertEqual(stable.read_text(), "stable installation")

    def test_invalid_downloads_cannot_replace_working_candidate(self):
        good, checksum = package()
        current = rc.install(good, checksum, self.root)
        bad, bad_hash = package(build="two", binary_version="1.2.0")
        wrong_platform, wrong_hash = package(build="three", platform="Different-architecture")
        traversal, traversal_hash = package(extra="../outside")
        for payload, digest in [(good + b"corrupt", checksum), (bad, bad_hash), (wrong_platform, wrong_hash), (traversal, traversal_hash)]:
            with self.subTest(digest=digest), self.assertRaises(ValueError):
                rc.install(payload, digest, self.root)
            self.assertEqual((self.root / "current").resolve(), current)
        self.assertFalse((self.root.parent / "outside").exists())
        self.assertEqual(list(self.root.glob(".stage-*")), [])

    def test_new_candidate_promotes_atomically_and_keeps_previous_build(self):
        first = rc.install(*package(), self.root)
        second = rc.install(*package(build="two"), self.root)
        self.assertEqual((self.root / "current").resolve(), second)
        self.assertTrue((first / "mlxtop").is_file())

    def test_real_current_directory_is_never_replaced(self):
        (self.root / "current").mkdir()
        with self.assertRaisesRegex(ValueError, "real current directory"):
            rc.install(*package(), self.root)
        self.assertTrue((self.root / "current").is_dir())

    def test_ssh_push_and_fetch_roundtrip(self):
        remote_home = self.root / "remote"
        local = self.root / "local"
        args = argparse.Namespace(host="omlx-test", port=2222, identity="key with spaces", dry_run=False)
        real_run = subprocess.run
        calls = []

        def ssh(command, **kwargs):
            if command[0] != "ssh":
                return real_run(command, **kwargs)
            calls.append(command)
            remote = shlex.split(command[-1])
            self.assertEqual(remote[:2], ["python3", "-c"])
            code = remote[2].replace("Path.home()", "Path(" + repr(str(remote_home)) + ")")
            return real_run([sys.executable, "-c", code], **kwargs)

        with patch.object(rc, "OUTPUT", local), patch.object(rc, "build", return_value=package()), patch.object(rc.subprocess, "run", side_effect=ssh):
            rc.push(args)
            rc.fetch(args)
            self.assertEqual((local / "current/mlxtop").read_bytes(), (remote_home / rc.REMOTE_ROOT / "current/mlxtop").read_bytes())
            # Repeating the same download should not fail or duplicate the build.
            rc.fetch(args)
        self.assertTrue(all(command[:6] == ["ssh", "-T", "-p", "2222", "-i", "key with spaces"] for command in calls))

    def test_server_build_is_available_to_remote_fetch(self):
        with patch.object(rc, "build", return_value=package()), patch.object(rc.Path, "home", return_value=self.root):
            self.assertEqual(rc.main(["build", "--stage-for-fetch"]), 0)
        self.assertTrue((self.root / rc.REMOTE_ROOT / "current/rc.tar.gz").is_file())
        self.assertTrue((self.root / rc.REMOTE_ROOT / "current/SHA256SUMS").is_file())

    def test_dry_run_does_not_build_or_connect(self):
        with patch.object(rc, "build") as build, patch.object(rc.subprocess, "run") as run:
            self.assertEqual(rc.main(["push", "omlx-test", "--dry-run"]), 0)
            self.assertEqual(rc.main(["fetch", "omlx-test", "--dry-run"]), 0)
            self.assertEqual(rc.main(["run", "omlx-test", "--once", "--dry-run"]), 0)
        build.assert_not_called()
        run.assert_not_called()

    def test_run_passes_a_recording_path_quoted(self):
        output = io.StringIO()
        with patch.object(rc.subprocess, "run") as run, redirect_stdout(output):
            self.assertEqual(rc.main(["run", "omlx-test", "--record", "~/a b.jsonl", "--dry-run"]), 0)
        run.assert_not_called()
        self.assertIn("--record", output.getvalue())
        self.assertIn("a b.jsonl", output.getvalue())


if __name__ == "__main__":
    unittest.main()
