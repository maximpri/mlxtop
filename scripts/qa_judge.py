#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""QA judge: a language model reviews every screen mlxtop draws against its UX rules.

The test suite already enforces the rules a program can check (the anomaly
colour only where the model is slowed, no noise phrases, panels and borders
whole). This asks a model the questions a program cannot: does each panel say
something a person can act on, is anything said twice, is anything cut off or
confusing. It renders the screens with
`cargo test qa_screens_for_the_model_judge -- --ignored`, sends each to an
OpenAI-compatible endpoint (a local oMLX, llama.cpp or LM Studio server works),
and fails when the model reports a blocker.

    QA_JUDGE_URL=http://127.0.0.1:8000/v1 python3 scripts/qa_judge.py
    python3 scripts/qa_judge.py --url http://studio.local:8080/v1 --model Qwen3.8-27B
    python3 scripts/qa_judge.py --dry-run          # print one prompt, send nothing

The API key, if the server needs one, is read from QA_JUDGE_API_KEY and is
never printed. Screens contain only what mlxtop shows: no prompts or output.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

RUBRIC = """You review screens of mlxtop, a terminal monitor whose one job is to find
what limits a local LLM's speed (memory, paging, disk, heat, the GPU, the
queue, the runtime) and say what to change. Judge the screen against these
rules, which its designers hold to:

1. Colour means impact. The anomaly colour (magenta) appears only while the
   model is slowed or stalled, on the samples of that slowdown and on the
   signal that explains it. Magenta on a healthy screen is a blocker.
2. Impact over readouts. A panel says what a reading means for the model
   (headroom left, time saved, what the disk is doing), not a raw rate that
   repeats its title. A rate shown twice in one panel is a major issue.
3. Each fact once. The same number or statement in two places on the
   screen is a minor issue; in one panel, major.
4. No status sentences where a chart belongs ("Light compression · below
   warning"); a short panel draws its data. Major.
5. Nothing cut off, overlapping a border, or truncated mid-word. Blocker if
   it hides a reading, major otherwise.
6. Labels a person understands without a manual; units on every number;
   no internal names, no "None", "NaN" or debug text. Major.
7. Advice names a change and the reading behind it. Vague advice is minor.

How to read a screen:
- The numbers at the left edge of a chart are its axis scale, not readings.
- An overlay (Diagnostics, the session summary, help) covers the panels
  under it by design: judge the overlay's own content, not what it hides.
- Before the first sample, a panel may say it has none yet ("No samples
  yet"): that is an empty state, not a status sentence.
- A "[magenta cells: …]" note under the screen lists what is drawn in the
  anomaly colour; "[state: …]" says whether the model is slowed.

Answer with JSON only, no prose around it:
{"verdict": "pass" | "issues" | "blocker",
 "issues": [{"severity": "blocker" | "major" | "minor",
             "rule": <rule number>, "where": "<panel or row>",
             "detail": "<what is wrong and why it matters>"}]}
An empty issue list means pass. Report only what you can see on this screen."""


def render_screens(directory):
    """Render every screen the judge covers into `directory`."""
    env = dict(os.environ, QA_SCREENS_DIR=str(directory))
    subprocess.run(
        ["cargo", "test", "--locked", "qa_screens_for_the_model_judge", "--", "--ignored", "--quiet"],
        cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL,
    )
    return sorted(Path(directory).glob("*.txt"))


def request(url, payload, api_key, timeout):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(url, data=data, method="POST" if data else "GET")
    req.add_header("Content-Type", "application/json")
    if api_key:
        req.add_header("Authorization", "Bearer " + api_key)
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.loads(response.read())


def first_model(base, api_key, timeout):
    listing = request(base.rstrip("/") + "/models", None, api_key, timeout)
    models = [entry.get("id") for entry in listing.get("data", []) if entry.get("id")]
    if not models:
        raise ValueError("the endpoint lists no models; pass --model")
    return models[0]


def parse_verdict(content):
    """The model's JSON verdict, tolerating code fences and a thinking block."""
    content = re.sub(r"<think>.*?</think>", "", content, flags=re.S)
    match = re.search(r"\{.*\}", content, flags=re.S)
    if not match:
        return {"verdict": "issues", "issues": [{"severity": "minor", "rule": 0, "where": "judge",
                                                 "detail": "the model gave no JSON verdict"}]}
    try:
        verdict = json.loads(match.group(0))
    except json.JSONDecodeError:
        return {"verdict": "issues", "issues": [{"severity": "minor", "rule": 0, "where": "judge",
                                                 "detail": "the model's verdict was not valid JSON"}]}
    issues = [issue for issue in verdict.get("issues", []) if isinstance(issue, dict)]
    if any(issue.get("severity") == "blocker" for issue in issues):
        verdict["verdict"] = "blocker"
    elif issues and verdict.get("verdict") == "pass":
        verdict["verdict"] = "issues"
    verdict["issues"] = issues
    return verdict


def judge(screen_text, base, model, api_key, timeout):
    reply = request(base.rstrip("/") + "/chat/completions", {
        "model": model,
        "temperature": 0,
        # Reasoning models think before they answer: leave room for both.
        "max_tokens": 8000,
        "messages": [
            {"role": "system", "content": RUBRIC},
            {"role": "user", "content": "Screen:\n```\n" + screen_text + "\n```"},
        ],
    }, api_key, timeout)
    message = reply["choices"][0]["message"]
    # Some servers put a reasoning model's answer beside its thinking.
    content = message.get("content") or message.get("reasoning_content") or ""
    return parse_verdict(content)


def report(results):
    """Markdown report, worst first, and whether any screen has a blocker."""
    order = {"blocker": 0, "issues": 1, "pass": 2}
    lines = ["# QA judge", "", "| Screen | Verdict | Issues |", "|---|---|---|"]
    for name, verdict in sorted(results, key=lambda r: (order.get(r[1]["verdict"], 1), r[0])):
        lines.append(f"| {name} | {verdict['verdict']} | {len(verdict['issues'])} |")
    for name, verdict in results:
        for issue in verdict["issues"]:
            lines.append(f"- **{issue.get('severity', '?')}** {name} · {issue.get('where', '')}: "
                         f"{issue.get('detail', '')} (rule {issue.get('rule', '?')})")
    blocked = any(verdict["verdict"] == "blocker" for _, verdict in results)
    return "\n".join(lines) + "\n", blocked


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", default=os.environ.get("QA_JUDGE_URL", "http://127.0.0.1:8000/v1"),
                        help="OpenAI-compatible base URL (default: $QA_JUDGE_URL or http://127.0.0.1:8000/v1)")
    parser.add_argument("--model", default=os.environ.get("QA_JUDGE_MODEL"), help="model id (default: the first listed)")
    parser.add_argument("--screens", type=Path, help="judge these rendered screens instead of rendering them")
    parser.add_argument("--only", help="judge only screens whose name contains this")
    parser.add_argument("--report", type=Path, help="also write the Markdown report here")
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument("--dry-run", action="store_true", help="print the first prompt and send nothing")
    args = parser.parse_args(argv)
    api_key = os.environ.get("QA_JUDGE_API_KEY")
    with tempfile.TemporaryDirectory() as scratch:
        screens = sorted(args.screens.glob("*.txt")) if args.screens else render_screens(scratch)
        if args.only:
            screens = [screen for screen in screens if args.only in screen.stem]
        if not screens:
            print("qa_judge: no screens to judge", file=sys.stderr)
            return 2
        if args.dry_run:
            print(RUBRIC + "\n\nScreen:\n```\n" + screens[0].read_text() + "\n```")
            print(f"\n({len(screens)} screens would be sent to {args.url})")
            return 0
        model = args.model or first_model(args.url, api_key, args.timeout)
        results = []
        for screen in screens:
            try:
                verdict = judge(screen.read_text(), args.url, model, api_key, args.timeout)
            except (OSError, ValueError, KeyError) as error:
                # One screen the judge could not answer is reported, not fatal.
                verdict = {"verdict": "issues", "issues": [{"severity": "minor", "rule": 0, "where": "judge",
                                                             "detail": f"no verdict: {type(error).__name__}"}]}
            print(f"{screen.stem:<32} {verdict['verdict']}", flush=True)
            results.append((screen.stem, verdict))
    text, blocked = report(results)
    print("\n" + text)
    if args.report:
        args.report.write_text(text)
    return 1 if blocked else 0


if __name__ == "__main__":
    sys.exit(main())
