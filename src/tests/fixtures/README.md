# Runtime contract fixtures

Splash snapshots project the fields consumed by mlxtop from upstream
`dev/tests/engine/status_golden.json` at tags 1.3.2, 1.4.0, 1.4.1 and 1.5.0:
https://github.com/incoai/splash (Apache-2.0, copyright Splash contributors).
The `instance` and `transport` envelopes use synthetic identifiers, following
`server/server.py::status` and `server/backend.py::status`. Unused identity,
model-layout and kernel sections are omitted. These are upstream test data,
not recordings from a running model.

oMLX fixtures model the documented JSON contracts in `omlx/admin/routes.py`,
`omlx/prefill_progress.py` and `omlx/engine/splash.py` at v0.7.1, v0.7.2 and
v0.7.3rc1: https://github.com/jundot/omlx (Apache-2.0). The relevant health,
admin-stats and active-model handlers are unchanged across those tags.
Identifiers are synthetic. In particular, Splash prefill rows expose `total`
without `prompt_tokens`, while native rows can carry prompt/cache metadata.
No prompts, replies, credentials, user paths or real model identifiers are stored.
