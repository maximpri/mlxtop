# SPDX-License-Identifier: MIT
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import qa_judge


class Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


def reply(content):
    return Response(json.dumps({"choices": [{"message": {"content": content}}]}).encode())


class QaJudgeTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.screens = Path(self.dir.name)
        (self.screens / "healthy-120x40.txt").write_text("┌ memory ┐\n[yellow cells: none]\n")
        (self.screens / "stall-120x40.txt").write_text("┌ generation ┐\n")

    def tearDown(self):
        self.dir.cleanup()

    def test_verdicts_tolerate_fences_and_thinking_and_a_blocker_wins(self):
        verdict = qa_judge.parse_verdict('<think>hmm</think>```json\n{"verdict": "pass", "issues": []}\n```')
        self.assertEqual(verdict["verdict"], "pass")
        verdict = qa_judge.parse_verdict(json.dumps({"verdict": "pass", "issues": [
            {"severity": "blocker", "rule": 1, "where": "paging", "detail": "yellow while healthy"}]}))
        self.assertEqual(verdict["verdict"], "blocker", "a blocker is never a pass")
        self.assertEqual(qa_judge.parse_verdict("no json here")["verdict"], "issues")
        self.assertEqual(qa_judge.parse_verdict("{not json}")["verdict"], "issues")

    def test_a_blocker_fails_the_run_and_the_report_lists_it(self):
        replies = [
            reply('{"verdict": "pass", "issues": []}'),
            reply('{"verdict": "blocker", "issues": [{"severity": "blocker", "rule": 5, '
                  '"where": "generation", "detail": "reading cut off"}]}'),
        ]
        report = self.screens / "report.md"
        output = io.StringIO()
        with patch.object(qa_judge.urllib.request, "urlopen", side_effect=replies) as urlopen, redirect_stdout(output):
            code = qa_judge.main(["--screens", str(self.screens), "--model", "m", "--url", "http://judge/v1",
                                  "--report", str(report)])
        self.assertEqual(code, 1)
        sent = json.loads(urlopen.call_args_list[0].args[0].data)
        self.assertEqual(sent["temperature"], 0)
        self.assertIn("Colour means impact", sent["messages"][0]["content"])
        text = report.read_text()
        self.assertIn("| stall-120x40 | blocker | 1 |", text)
        self.assertIn("reading cut off (rule 5)", text)
        self.assertLess(text.index("stall-120x40"), text.index("healthy-120x40"), "worst first")

    def test_passing_screens_pass_and_the_key_stays_private(self):
        output = io.StringIO()
        with patch.dict("os.environ", {"QA_JUDGE_API_KEY": "secret-key"}), \
                patch.object(qa_judge.urllib.request, "urlopen",
                             side_effect=lambda *a, **k: reply('{"verdict": "pass", "issues": []}')) as urlopen, \
                redirect_stdout(output):
            code = qa_judge.main(["--screens", str(self.screens), "--model", "m", "--only", "healthy"])
        self.assertEqual(code, 0)
        self.assertEqual(urlopen.call_count, 1, "--only narrows the screens")
        self.assertEqual(urlopen.call_args.args[0].get_header("Authorization"), "Bearer secret-key")
        self.assertNotIn("secret-key", output.getvalue())

    def test_an_answer_beside_thinking_counts_and_a_failed_screen_is_reported(self):
        thinking = Response(json.dumps({"choices": [{"message": {
            "content": None, "reasoning_content": '{"verdict": "pass", "issues": []}'}}]}).encode())
        with patch.object(qa_judge.urllib.request, "urlopen",
                          side_effect=[thinking, OSError("connection reset")]), redirect_stdout(io.StringIO()):
            code = qa_judge.main(["--screens", str(self.screens), "--model", "m"])
        self.assertEqual(code, 0, "an unanswered screen is an issue, not a blocker")

    def test_the_first_listed_model_is_used_and_dry_run_sends_nothing(self):
        listing = Response(json.dumps({"data": [{"id": "qwen"}, {"id": "other"}]}).encode())
        with patch.object(qa_judge.urllib.request, "urlopen", return_value=listing):
            self.assertEqual(qa_judge.first_model("http://judge/v1", None, 5), "qwen")
        with patch.object(qa_judge.urllib.request, "urlopen",
                          return_value=Response(b'{"data": []}')), self.assertRaises(ValueError):
            qa_judge.first_model("http://judge/v1", None, 5)
        output = io.StringIO()
        with patch.object(qa_judge.urllib.request, "urlopen") as urlopen, redirect_stdout(output):
            self.assertEqual(qa_judge.main(["--screens", str(self.screens), "--dry-run"]), 0)
        urlopen.assert_not_called()
        self.assertIn("2 screens would be sent", output.getvalue())
        with redirect_stdout(io.StringIO()):
            self.assertEqual(qa_judge.main(["--screens", str(self.screens), "--only", "nothing"]), 2)


if __name__ == "__main__":
    unittest.main()
