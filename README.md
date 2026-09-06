# dsh desktop

[中文](README.zh.md)

**dsh desktop** is an open-source [Tauri 2](https://v2.tauri.app) desktop client
for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`).

It follows three design rules:

1. **No dsh code in this repository.** The harness is never vendored,
   forked, or patched here.
2. **Install on demand, through the official channel.** On startup the client
   checks whether `dsh` exists on the system; when it is missing it runs the
   official install (`npm install -g @deepseek-ai/dsh`) and composes a
   dedicated **`desktop` profile** (`$DSH_HOME/profiles/desktop`) with the
   official `dsh plugin` command.
3. **Client ↔ dsh communication is a dsh plugin.** All shell-originated calls
   into the harness go through **dsh-desktop-bridge**, a plugin this project
   authors and installs into the `desktop` profile — never through scraped
   logs or harness internals.

## How it works

```
┌─────────────────────────── dsh desktop (this repo) ───────────────────────────┐
│  Tauri 2 shell (Rust)                                                          │
│    ┌──────────────────────────────────────────────┐   ┌───────────────────┐   │
│    │ 1. probe `dsh` → 2. install if missing (npm) │   │                   │   │
│    │ 3. ensure `desktop` profile (dsh plugin)     │   │   dsh web UI      │   │
│    │ 4. spawn `dsh --profile desktop --no-open`   ├──▶│   (harness's own  │   │
│    │ 5. read stdout markers, navigate to          │   │   web app)       │   │
│    │    authenticated URL                         │   │                   │   │
│    └───────┬───────────────────────────▲──────────┘   └───────────────────┘   │
└────────────┼───────────────────────────┼──────────────────────────────────────┘
             │ stdout: `dsh-desktop-ready`│   HTTP (loopback control API)
             │        `dsh web: …?token=` │
             ▼                            │
┌──────────────────────────────────────────────────────────────────────────────┐
│  dsh (installed from npm, outside this repo)                                  │
│    profile `desktop` = bundles, in order:                                     │
│      @deepseek-ai/dsh-base        (official core)                             │
│      @deepseek-ai/dsh-web-app     (official web UI + webserver)               │
│      dsh-desktop-bridge           (this repo's plugin, packages/bridge)       │
│    the bridge plugin prints a machine-readable ready line and serves          │
│    GET /api/desktop/health · /api/desktop/info on 127.0.0.1                   │
└──────────────────────────────────────────────────────────────────────────────┘
```

The **UI is the harness's own web app** — the shell opens it at the
authenticated URL the harness announces — while **control-plane traffic from
the shell** (locating the backend, health, info, future RPC) goes through the
plugin-registered API. Window close sends the backend a graceful SIGTERM
(5 s drain, then SIGKILL).

## Repository layout

```
packages/bridge/     dsh-desktop-bridge — the dsh plugin (bundle) that opens
                     the control channel. Zero runtime dependencies, imports
                     nothing from the harness.
apps/desktop/        The Tauri 2 client: bootstrap UI (ui/), Rust shell
                     (src-tauri/src/main.rs), macOS/windows packaging.
```

## Requirements

- **Node.js ≥ 22.19** and **pnpm** (dsh and the profile tooling need both;
  the client installs dsh itself, pnpm is only needed if you compose or
  extend the profile by hand)
- **Rust** toolchain to build the desktop app (see below)

## Development

```sh
pnpm install                    # workspace deps (@tauri-apps/cli, bridge toolchain)
pnpm build:bridge               # compile packages/bridge → lib/

# one of:
pnpm desktop                    # tauri dev (debug build, hot reload on Rust edits)
pnpm desktop:build              # tauri build → release bundles
```

Environment overrides used by the shell:

| Variable | Meaning |
|---|---|
| `DSH_DESKTOP_DSH` | Use this dsh binary/path instead of probing PATH |
| `DSH_DESKTOP_BRIDGE_PACKAGE` | Bridge install spec for the profile (default: the published `dsh-desktop-bridge`; point it at a packed tarball before the first npm release) |
| `DSH_HOME` | Honored for the profile location (default `~/.dsh`) |

Logs live in `~/.dsh-desktop/` (`backend.log`, `install.log`,
`profile-add.log`, `dsh-desktop.log`).

## The `desktop` profile and the plugin

Everything the shell does by hand can be done from a terminal:

```sh
# ensure dsh (official channel)
npm install -g @deepseek-ai/dsh

# compose the desktop profile (first use initializes it automatically)
dsh plugin --profile desktop add @deepseek-ai/dsh-web-app
dsh plugin --profile desktop add dsh-desktop-bridge

# inspect the composed tree without booting
dsh --profile desktop --dump-config

# boot the same way the client does
dsh --profile desktop --no-open
```

Plugin behavior and the control-API contract are documented in
[`packages/bridge/README.md`](packages/bridge/README.md). dsh plugin
authoring follows DeepSeek Harness's official
[publish tutorial](https://deepseek-harness.github.io/deepseek-harness/user/develop/basic/publish.html).

## Status

Working, minimal v0.1: install-on-demand, profile composition, boot, and the
plugin control channel are implemented and verified end to end. Release
bundling (dmg/nsis) and code-signing are not set up yet.

## License

MIT. This project is an independent, community effort and is not affiliated
with or endorsed by DeepSeek. `dsh` / DeepSeek Harness are developed by
DeepSeek AI and are installed from their official npm channel, never bundled
here.
