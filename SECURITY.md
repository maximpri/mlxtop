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

Include the affected version, macOS version, runtime/provider, reproduction
steps, expected impact and any suggested mitigation. Remove API keys, prompts,
model output, hostnames and other private workload data.

The most sensitive boundaries are provider authentication, intentionally remote
provider endpoints, parsing of local process/log data and the optional SSH
deployment helper. mlxtop sends provider credentials only to loopback endpoints
unless `MLXTOP_ALLOW_REMOTE_AUTH=1` is explicitly set. Remote HTTP transport is
not encrypted by mlxtop; use a trusted tunnel or protected network.

mlxtop writes local diagnostic logs to `~/Library/Logs/mlxtop/mlxtop.log` by
default. These logs may contain system counters and model/provider names, but
the logger intentionally excludes prompts, model output, request bodies and
provider API keys. Remove or redact the relevant log before sharing it.

Reports will be acknowledged and assessed on a best-effort basis. Please allow
time to investigate before public disclosure.
