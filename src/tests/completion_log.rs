// SPDX-License-Identifier: MIT
use super::*;

/// Two lines from the owner's oMLX log, Splash serving: an answer not streamed, and a
/// streamed one, for which oMLX also logs the time to its first token.
const WHOLE_REQUEST: &str = "2026-11-15 02:38:45,641 - omlx.server - INFO - [-] - Chat completion: model=Qwen3.8-27B-oQ4e-mtp, 602 tokens in 8.26s (72.9 tok/s), prompt: 1186, finish_reason=stop, max_tokens=3072, request_max_tokens=3072";
const DECODE_ONLY: &str = "2026-11-13 13:48:29,070 - omlx.server - INFO - [-] - Chat completion: model=Qwen3.8-27B-oQ4e-mtp, 617 tokens in 113.49s (39.5 tok/s), prompt: 33587, finish_reason=tool_calls, max_tokens=40960, request_max_tokens=40960, stream_model_ttft=97.88s, stream_visible_ttft=97.88s";

#[test]
fn a_rate_logged_over_the_whole_request_is_no_generation_speed() {
    // 602 tokens over 8.26 s is 72.9 tok/s: the wait for the first token is in it.
    let whole = parse_llm_completion_line(WHOLE_REQUEST).unwrap();
    assert_eq!(whole.tokens_per_second, None);
    assert_eq!(whole.output_tokens, Some(602));
    assert_eq!(whole.prompt_tokens, Some(1186));
    // 617 tokens over 113.49 s is 5.4 tok/s; 39.5 is the speed after the first token, 97.9 s
    // in.
    let decode = parse_llm_completion_line(DECODE_ONLY).unwrap();
    assert_eq!(decode.tokens_per_second, Some(39.5));
}

#[test]
fn only_a_rate_clearly_faster_than_the_whole_requests_is_generation() {
    assert_eq!(generation_rate(120, 3.0, 60.0), Some(60.0));
    // Within the log's rounding of the whole request's rate.
    assert_eq!(generation_rate(120, 2.0, 60.0), None);
    assert_eq!(generation_rate(120, 2.0, 61.0), None);
    // No time, or a rate that is no number, says nothing.
    assert_eq!(generation_rate(120, 0.0, 60.0), None);
    assert_eq!(generation_rate(120, 3.0, f64::NAN), None);
}
