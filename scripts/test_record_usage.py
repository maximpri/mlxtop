# SPDX-License-Identifier: MIT
import concurrent.futures
import json
from pathlib import Path
import tempfile
import unittest

from record_usage import append_usage, usage_record


class UsageWriterTests(unittest.TestCase):
    def test_all_runtimes_and_response_content_is_excluded(self):
        for provider in ("omlx", "mlx_lm.server", "llama-server", "koboldcpp", "localai", "vllm", "sglang", "jan", "gpt4all", "mlx-serve", "splash", "tensorfold", "strata", "magnitude"):
            response = {"usage": {"prompt_tokens": 1024, "completion_tokens": 12},
                        "messages": ["private"], "choices": ["secret"], "api_key": "key"}
            record = usage_record(provider, response, request_id="req-1", observed_at=1700000000)
            self.assertEqual(record["usage"]["prompt_tokens"], 1024)
            self.assertEqual(set(record), {"provider", "request_id", "observed_at", "usage"})
        ollama = usage_record("ollama", {"done": True, "prompt_eval_count": 80,
                             "eval_count": 4, "response": "private"})
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

    def test_llama_server_timings_are_read_whatever_runtime_sent_them(self):
        # A final reply's `timings`, as llama.cpp v1.0.0 server_slot_stats::to_json
        # writes them; Splash and proxies send the same shape.
        timings = {"cache_n": 24576, "prompt_n": 6144, "prompt_ms": 7680.0,
                   "prompt_per_token_ms": 1.25, "prompt_per_second": 800.0,
                   "predicted_n": 512, "predicted_ms": 12800.0,
                   "predicted_per_token_ms": 25.0, "predicted_per_second": 40.0,
                   "draft_n": 600, "draft_n_accepted": 420}
        for provider in ("llama.cpp", "splash", "omlx", "tensorfold"):
            record = usage_record(provider, {
                "usage": {"prompt_tokens": 30720, "completion_tokens": 512},
                "timings": timings, "choices": [{"message": {"content": "private"}}]})
            self.assertEqual(record["timings"], {"output_tokens_per_second": 40.0,
                                                 "prompt_tokens_per_second": 800.0})
            self.assertEqual(record["usage"]["prompt_tokens_details"], {"cached_tokens": 24576})
            self.assertNotIn("private", json.dumps(record))
        # A usage count of cached tokens wins over the timings'.
        record = usage_record("llama.cpp", {
            "usage": {"prompt_tokens": 100, "prompt_tokens_details": {"cached_tokens": 10}},
            "timings": {"cache_n": 60}})
        self.assertEqual(record["usage"]["prompt_tokens_details"], {"cached_tokens": 10})
        # A prompt speed of zero is a time the server did not have.
        record = usage_record("llama.cpp", {"usage": {"prompt_tokens": 1},
                                            "timings": {"prompt_per_second": 0.0}})
        self.assertNotIn("timings", record)

    def test_ollama_prompt_speed_counts_uncached_tokens_and_load_is_kept(self):
        # A final /api/chat reply: api.Metrics in Ollama v0.40.16, in nanoseconds.
        record = usage_record("ollama", {
            "done": True, "message": {"content": "private"},
            "total_duration": 9_500_000_000, "load_duration": 3_250_000_000,
            "prompt_eval_count": 2048, "prompt_eval_cached_count": 1024,
            "prompt_eval_duration": 2_000_000_000,
            "eval_count": 100, "eval_duration": 4_000_000_000})
        self.assertEqual(record["timings"], {"output_tokens_per_second": 25.0,
                                             "prompt_tokens_per_second": 512.0,
                                             "model_load_ms": 3250})
        self.assertNotIn("private", json.dumps(record))
        # A prompt read entirely from the cache was not timed.
        cached = usage_record("ollama", {"prompt_eval_count": 64, "prompt_eval_cached_count": 64,
                                         "prompt_eval_duration": 1_000_000})
        self.assertNotIn("timings", cached)

    def test_lm_studio_reports_a_model_load_only_after_one(self):
        loaded = usage_record("lmstudio", {"stats": {"input_tokens": 40, "total_output_tokens": 3,
                                                     "model_load_time_seconds": 4.5}})
        self.assertEqual(loaded["timings"], {"model_load_ms": 4500})
        warm = usage_record("lmstudio", {"stats": {"input_tokens": 40, "total_output_tokens": 3}})
        self.assertNotIn("timings", warm)
        # A record already written by this helper keeps its fields.
        again = usage_record("lmstudio", {"stats": {"input_tokens": 1},
                                          "timings": {"model_load_ms": 1800,
                                                      "prompt_tokens_per_second": 99.5}})
        self.assertEqual(again["timings"], {"prompt_tokens_per_second": 99.5,
                                            "model_load_ms": 1800})

    def test_omlx_usage_gives_prompt_speed_over_computed_tokens_and_its_load(self):
        # A streamed chat's final usage chunk, as oMLX v0.8.0rc1 server.py
        # writes it: `Usage` (api/openai_models.py) extended with timings in
        # seconds, its None fields left out. The prompt speed counts only the
        # tokens oMLX computed; a load is sent only when it took over a second.
        response = {"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [],
                    "model": "Qwen3.8-27B-oQ4e-mtp",
                    "usage": {"prompt_tokens": 14479, "completion_tokens": 812,
                              "total_tokens": 15291,
                              "prompt_tokens_details": {"cached_tokens": 0},
                              "completion_tokens_details": {"reasoning_tokens": 0},
                              "model_load_duration": 6.48, "time_to_first_token": 55.31,
                              "total_time": 61.91, "prompt_eval_duration": 48.62,
                              "generation_duration": 6.6, "prompt_tokens_per_second": 297.8,
                              "generation_tokens_per_second": 123.03}}
        record = usage_record("omlx", response)
        self.assertEqual(record["timings"], {"prompt_tokens_per_second": 297.8,
                                             "model_load_ms": 6480})
        self.assertEqual(record["usage"]["prompt_tokens_details"], {"cached_tokens": 0})
        self.assertEqual(record["model"], "Qwen3.8-27B-oQ4e-mtp")
        warm = {**response, "usage": {key: value for key, value in response["usage"].items()
                                      if key != "model_load_duration"}}
        self.assertEqual(usage_record("omlx", warm)["timings"],
                         {"prompt_tokens_per_second": 297.8})
        # A prompt served whole from the cache was not timed.
        cached = {**warm, "usage": {**warm["usage"], "prompt_tokens_per_second": 0.0}}
        self.assertNotIn("timings", usage_record("omlx", cached))
        # oMLX's names mean nothing from another runtime.
        self.assertNotIn("timings", usage_record("mlx-lm", response))
        for value in (-1, True, float("nan"), "6.5s"):
            for key in ("prompt_tokens_per_second", "model_load_duration"):
                with self.assertRaises(ValueError):
                    usage_record("omlx", {**response, "usage": {**response["usage"], key: value}})

    def test_invalid_prompt_and_load_timings_are_rejected(self):
        for value in (-1, True, float("nan"), float("inf"), "bad"):
            for timings in ({"prompt_per_second": value}, {"prompt_tokens_per_second": value},
                            {"predicted_per_second": value}):
                with self.assertRaises(ValueError):
                    usage_record("splash", {"usage": {"prompt_tokens": 1}, "timings": timings})
            with self.assertRaises(ValueError):
                usage_record("lmstudio", {"stats": {"input_tokens": 1,
                                                    "model_load_time_seconds": value}})
        for value in (-1, 1.5, True, "1s"):
            for key in ("load_duration", "prompt_eval_duration"):
                with self.assertRaises(ValueError):
                    usage_record("ollama", {"prompt_eval_count": 1, key: value})
            with self.assertRaises(ValueError):
                usage_record("mlx-lm", {"usage": {"prompt_tokens": 1},
                                        "timings": {"model_load_ms": value}})
        with self.assertRaises(ValueError):
            usage_record("lmstudio", {"stats": {"input_tokens": 1,
                                                "model_load_time_seconds": 2.0**64}})
        with self.assertRaises(ValueError):
            usage_record("splash", {"usage": {"prompt_tokens": 1}, "timings": {"cache_n": 2}})

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
