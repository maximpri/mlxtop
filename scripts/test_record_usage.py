# SPDX-License-Identifier: MIT
import concurrent.futures
import json
from pathlib import Path
import tempfile
import unittest

from record_usage import append_usage, usage_record


class UsageWriterTests(unittest.TestCase):
    def test_all_runtimes_and_response_content_is_excluded(self):
        for provider in ("omlx", "mlx_lm.server", "llama-server", "koboldcpp", "localai", "vllm", "sglang", "jan", "gpt4all", "mlx-serve"):
            response = {"usage": {"prompt_tokens": 1024, "completion_tokens": 12},
                        "messages": ["private"], "choices": ["secret"], "api_key": "key"}
            record = usage_record(provider, response, request_id="req-1", observed_at=1700000000)
            self.assertEqual(record["usage"]["prompt_tokens"], 1024)
            self.assertEqual(set(record), {"provider", "request_id", "observed_at", "usage"})
        ollama = usage_record("ollama", {"done": True, "prompt_eval_count": 80,
                             "eval_count": 4, "response": "private", "prompt_eval_duration": 123})
        self.assertEqual(ollama["usage"], {"prompt_tokens": 80, "completion_tokens": 4})
        self.assertNotIn("timings", ollama)
        lm = usage_record("lmstudio", {"stats": {"input_tokens": 40, "total_output_tokens": 3},
                                       "model_instance_id": "model"})
        self.assertEqual(lm["usage"]["completion_tokens"], 3)
        self.assertEqual(lm["model"], "model")

    def test_cache_zero_and_explicit_timing(self):
        record = usage_record("localai", {"usage": {"input_tokens": 0, "output_tokens": 0,
                              "input_tokens_details": {"cached_tokens": 0}}}, ttft_ms=5)
        self.assertEqual(record["usage"]["prompt_tokens_details"]["cached_tokens"], 0)
        self.assertEqual(record["timings"]["time_to_first_token_ms"], 5)

    def test_invalid_and_partial_responses_are_not_written(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "usage.jsonl"
            for response in ({}, {"done": False, "prompt_eval_count": 50},
                             {"usage": {"prompt_tokens": -1}}, {"usage": {"prompt_tokens": True}},
                             {"usage": {"prompt_tokens": 1.5}}, {"usage": {"prompt_tokens": 2**64}},
                             {"usage": {"prompt_tokens": 1, "cached_tokens": 2}}):
                with self.assertRaises(ValueError):
                    append_usage(path, "ollama", response)
            self.assertFalse(path.exists())

    def test_native_timings_are_normalized_without_response_content(self):
        ollama = usage_record("ollama", {"done": True, "prompt_eval_count": 100,
            "prompt_eval_cached_count": 60, "eval_count": 80, "eval_duration": 2_000_000_000,
            "total_duration": 99_000_000_000, "response": "private"})
        self.assertEqual(ollama["timings"], {"output_tokens_per_second": 40.0})
        self.assertEqual(ollama["usage"]["prompt_tokens_details"]["cached_tokens"], 60)
        lm = usage_record("lmstudio", {"stats": {"input_tokens": 100, "total_output_tokens": 80,
            "tokens_per_second": 32.5, "time_to_first_token_seconds": 0.125}, "output": "private"})
        self.assertEqual(lm["timings"], {"output_tokens_per_second": 32.5, "time_to_first_token_ms": 125})
        self.assertNotIn("private", json.dumps(lm))
        llama = usage_record("llama.cpp", {"usage": {"prompt_tokens": 1},
            "timings": {"predicted_per_second": 12.5, "time_to_first_token_ms": 25}})
        self.assertEqual(llama["timings"]["output_tokens_per_second"], 12.5)
        zero = usage_record("ollama", {"prompt_eval_count": 0, "eval_count": 0, "eval_duration": 0})
        self.assertNotIn("timings", zero)

    def test_invalid_native_timings_are_rejected(self):
        for value in (-1, True, float("nan"), float("inf"), "bad"):
            for key in ("tokens_per_second", "time_to_first_token_seconds"):
                with self.assertRaises(ValueError):
                    usage_record("lmstudio", {"stats": {"input_tokens": 1, key: value}})
        with self.assertRaises(ValueError):
            usage_record("ollama", {"prompt_eval_count": 1, "eval_duration": -1})
        with self.assertRaises(ValueError):
            usage_record("vllm", {"usage": {"prompt_tokens": 1}, "timings": []})

    def test_concurrent_appends_are_complete_and_unique(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "usage.jsonl"
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                ids = list(pool.map(lambda _: append_usage(path, "mlx-lm", {
                    "usage": {"prompt_tokens": 123, "completion_tokens": 4}}), range(40)))
            records = [json.loads(line) for line in path.read_text().splitlines()]
            self.assertEqual(len(records), 40)
            self.assertEqual(len(set(ids)), 40)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
