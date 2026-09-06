# dsh-desktop-bridge

A [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (dsh)
plugin — a **bundle** — that opens a control-plane channel between the
[dsh desktop](../..) client and a running harness tree.

It contains no dsh source code and imports nothing from the harness. It is an
ordinary third-party Cordis function plugin that runs *inside* the booted
tree, installed as a profile dependency through dsh's official plugin
mechanism:

```sh
dsh plugin --profile desktop add dsh-desktop-bridge
```

## What it provides

### 1. A loopback HTTP control API

Bound to `127.0.0.1` only (configurable via the bundle row's `config`; `port:
0` = ephemeral). Read-only endpoints:

| Endpoint | Response |
|---|---|
| `GET /api/desktop/health` | `{"ok":true,"plugin":"dsh-desktop-bridge","version":"0.1.0"}` |
| `GET /api/desktop/info` | `ok`, plugin name/version, `pid`, `platform`, `arch`, `node`, `startedAt`, `control.host/port`, `dshHome` |

### 2. A machine-readable ready announcement

When the server is listening, the plugin prints one line to stdout:

```
dsh-desktop-ready {"port":44123,"host":"127.0.0.1","version":"0.1.0"}
```

The desktop client captures the harness process's stdout and parses this
line to discover the control API — no scraping of human-oriented logs.

## API contract notes

- Responses are `application/json`, `no-store`, with `X-Content-Type-Options:
  nosniff`.
- State-changing endpoints do not exist yet; when they are added they will
  require an explicit client token so a random webpage cannot drive the local
  harness through the loopback socket (CSRF).
- The server closes on plugin-scope `dispose` and on process exit.

## Install notes

This bundle is loaded by the `desktop` profile that `dsh-desktop` composes:
`@deepseek-ai/dsh-base` → `@deepseek-ai/dsh-web-app` → `dsh-desktop-bridge`.
A package without the `dsh.bundle` manifest key would install as a plain
dependency and never activate as a layer.
