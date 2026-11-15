# SPDX-License-Identifier: MIT
"""The JSON output as a script sees it: `mlxtop --once --json` through
`python3 -m json.tool` and jq, and `mlxtop stream` ending quietly when its
reader goes away or a signal arrives. Runs the built binary (MLXTOP_BIN,
else target/release/mlxtop, else target/debug/mlxtop) and skips without one.
"""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


def built_binary():
    named = os.environ.get("MLXTOP_BIN")
    candidates = [Path(named)] if named else [
        ROOT / "target" / "release" / "mlxtop",
        ROOT / "target" / "debug" / "mlxtop",
    ]
    return next((path for path in candidates
                 if path.is_file() and os.access(path, os.X_OK)), None)


BINARY = built_binary()


@unittest.skipIf(BINARY is None, "build mlxtop first (cargo build --release) or set MLXTOP_BIN")
@unittest.skipIf(os.name == "nt", "the stream's signals are Unix's")
class JsonOutputTests(unittest.TestCase):
    def setUp(self):
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        # A home of its own: no user config, no runtime key, no log in ~.
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("MLXTOP_")}
        self.env["HOME"] = home.name
        self.env["MLXTOP_LOG_PATH"] = os.path.join(home.name, "mlxtop.log")

    def run_mlxtop(self, *args):
        return subprocess.run([str(BINARY), *args], capture_output=True,
                              env=self.env, timeout=60)

    def test_once_json_is_one_object_that_json_tool_reads(self):
        result = self.run_mlxtop("--once", "--json", "-i", "1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b"")
        self.assertEqual(result.stdout.count(b"\n"), 1)
        tool = subprocess.run([sys.executable, "-m", "json.tool"], input=result.stdout,
                              capture_output=True, timeout=30)
        self.assertEqual(tool.returncode, 0, tool.stderr)
        snapshot = json.loads(result.stdout)
        self.assertEqual(snapshot["schema_version"], 1)
        self.assertEqual(snapshot["host"]["source"], "host")
        self.assertIn(snapshot["verdict"]["severity"], ("ok", "warning", "critical", "neutral"))
        self.assertLess(len(result.stdout), 4096)

    def test_once_json_passes_jq(self):
        jq = shutil.which("jq")
        if jq is None:
            self.skipTest("jq is not installed")
        result = self.run_mlxtop("--once", "--json", "-i", "1")
        checked = subprocess.run(
            [jq, "-e", '.schema_version == 1 and (.verdict.state | type == "string")'
                       ' and .runtime.source == "runtime"'],
            input=result.stdout, capture_output=True, timeout=30)
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_a_schema_it_does_not_write_is_named(self):
        result = self.run_mlxtop("stream", "--schema", "2")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"unsupported JSON schema 2: this mlxtop writes schema 1", result.stderr)

    def test_stream_ends_quietly_once_head_has_read_enough(self):
        bash = shutil.which("bash")
        if bash is None:
            self.skipTest("bash is not installed")
        # The pipeline's status is mlxtop's own, not head's.
        result = subprocess.run(
            [bash, "-c", '"$0" stream -i 1 | head -3; exit "${PIPESTATUS[0]}"', str(BINARY)],
            capture_output=True, env=self.env, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b"")
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), 3)
        for line in lines:
            self.assertEqual(json.loads(line)["schema_version"], 1)

    def test_stream_ends_quietly_on_a_signal(self):
        for stop in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            with self.subTest(signal=stop.name):
                process = subprocess.Popen([str(BINARY), "stream", "-i", "1"],
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                           env=self.env)
                try:
                    first = process.stdout.readline()
                    process.send_signal(stop)
                    rest, err = process.communicate(timeout=30)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.communicate()
                self.assertEqual(json.loads(first)["schema_version"], 1)
                self.assertEqual(process.returncode, 0, err)
                self.assertEqual(err, b"")
                # A signal ends the stream between lines, never inside one.
                for line in rest.splitlines():
                    json.loads(line)


if __name__ == "__main__":
    unittest.main()
