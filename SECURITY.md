# Security policy

## Supported versions

Security fixes are made on the current default branch and included in the next
release. Older releases may not receive backports while the project is maintained
by a small team.

## Reporting a vulnerability

Do not disclose a suspected vulnerability in a public issue, discussion, log or
screenshot. Use GitHub's private vulnerability reporting feature from the
repository's **Security** tab. If private reporting is unavailable, open a public
issue containing no sensitive details and ask the maintainer for a private
contact channel.

Include the affected version, operating system and version, runtime/provider, reproduction
steps, expected impact and any suggested mitigation. Remove API keys, prompts,
model output, hostnames and other private workload data.

The most sensitive boundaries are provider authentication, intentionally remote
provider endpoints, parsing of local process/log data and the optional SSH
deployment helper. mlxtop sends provider credentials only to loopback endpoints
unless `MLXTOP_ALLOW_REMOTE_AUTH=1` is explicitly set. Native provider adapters support verified HTTPS and do not follow redirects or
environment proxies. Plain HTTP, including the existing oMLX transport, is not
encrypted; use HTTPS, a trusted tunnel or a protected network for remote access.

`mlxtop serve`, the only mode that listens on a socket, is opt-in and binds
loopback addresses only; it refuses any other `--listen` address. It answers
only requests whose Host header names a loopback host (against DNS
rebinding), only GET and HEAD, with request heads of at most 8 KiB within
2 seconds, and with no CORS headers. It accepts no input beyond the request
line and headers and changes nothing. Any local user or process can read its
answers, which name the runtime and model; reach it from another machine
through an SSH tunnel.

mlxtop writes local diagnostic logs to `~/Library/Logs/mlxtop/mlxtop.log` on
macOS, `~/.local/state/mlxtop/mlxtop.log` on Linux and
`%LOCALAPPDATA%\mlxtop\mlxtop.log` on Windows. These logs may contain system counters and model/provider names, but
the logger intentionally excludes prompts, model output, request bodies and
provider API keys. Remove or redact the relevant log before sharing it.
A `--record` file holds the machine's readings and the runtime's request ids,
model names and token counts, never prompts, output or process command lines,
and is created readable by its owner only (recordings made by 3.3.0 do contain
runtime command lines).
The usual speeds mlxtop keeps across sessions are in
`~/Library/Application Support/mlxtop/baselines.json` on macOS,
`~/.local/state/mlxtop/baselines.json` (or under `$XDG_STATE_HOME`) on Linux
and `%LOCALAPPDATA%\mlxtop\baselines.json` on Windows: speeds, the machine's
model identifier, chip and RAM, and runtime, model and version names, never
prompts, output, request ids, command lines or process names. The file is
readable by its owner only, in a folder only its owner can open, is never
sent anywhere, and is turned off by `"baselines": false` (Settings).

`mlxtop update` and the install scripts check each download against the
release's `SHA256SUMS`. That detects a damaged download; because the checksum
file comes from the same release, authenticity rests on TLS and on the GitHub
repository itself.

Reports will be acknowledged and assessed on a best-effort basis. Please allow
time to investigate before public disclosure.
