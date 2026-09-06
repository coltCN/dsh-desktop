/**
 * dsh-desktop-bridge — control-plane channel between the dsh desktop client
 * and a running DeepSeek Harness tree.
 *
 * This package is a dsh **bundle** (see the `dsh.bundle` manifest key): when a
 * profile lists `dsh-desktop-bridge` in `dsh.profile.bundles`, the cordis
 * patch in `cordis.patch.yml` inserts this plugin into the composed tree. It
 * carries no dsh source code and imports nothing from the harness — it is an
 * ordinary third-party Cordis function plugin (named exports `name` and
 * `apply`, no default export).
 *
 * What it does, once loaded:
 *
 * 1. Starts a loopback-only HTTP server (default `127.0.0.1:0` → ephemeral
 *    port) exposing the read-only control endpoints documented in README.md.
 * 2. Prints one machine-readable announcement to stdout when the server is
 *    listening:
 *
 *        dsh-desktop-ready {"port":44123,"host":"127.0.0.1","version":"0.1.0"}
 *
 *    The desktop client captures the harness process's stdout and uses this
 *    line (instead of scraping human log output) to learn where the control
 *    API lives.
 *
 * The server closes on the plugin scope's `dispose` event and on process
 * exit, so a graceful dsh shutdown never leaks the port.
 *
 * @module dsh-desktop-bridge
 */

import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { homedir } from 'node:os'
import { join } from 'node:path'
import http from 'node:http'
import type { IncomingMessage, ServerResponse } from 'node:http'

const require = createRequire(import.meta.url)

/** Package metadata read from the installed package.json — never hardcoded. */
const PACKAGE = JSON.parse(readFileSync(require.resolve('../package.json'), 'utf8')) as {
  name?: string
  version?: string
}

export const name = PACKAGE.name ?? 'dsh-desktop-bridge'

/** Config accepted from the bundle row's `config` key (see cordis.patch.yml). */
export interface BridgeConfig {
  /** Loopback host to bind. Keep this on 127.0.0.1. */
  host?: string
  /** Port to bind; 0 = kernel-assigned ephemeral port. */
  port?: number
}

/** The minimal piece of a Cordis plugin context this plugin touches. */
interface PluginContext {
  /** Subscribe to the plugin scope's dispose event to release resources. */
  on?(event: string, listener: () => void): unknown
}

/** The version of the plugin that is actually installed. */
function installedVersion(): string | undefined {
  return PACKAGE.version
}

/** Write a JSON response with hardening headers. */
function json(response: ServerResponse, status: number, body: Record<string, unknown>): void {
  const payload = JSON.stringify(body)
  response.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    'x-content-type-options': 'nosniff',
  })
  response.end(payload)
}

function jsonError(response: ServerResponse, status: number, error: string): void {
  json(response, status, { ok: false, error })
}

/** Read-only routes answering control-plane queries from the desktop client. */
function handleRequest(plugin: PluginRuntime) {
  return (request: IncomingMessage, response: ServerResponse): void => {
    const url = request.url ?? '/'
    const path = url.split('?')[0] ?? '/'
    if (request.method !== 'GET') {
      jsonError(response, 405, 'method not allowed')
      return
    }
    switch (path) {
      case '/api/desktop/health': {
        json(response, 200, { ok: true, plugin: plugin.name, version: plugin.version })
        return
      }
      case '/api/desktop/info': {
        json(response, 200, {
          ok: true,
          plugin: plugin.name,
          version: plugin.version,
          pid: plugin.pid,
          platform: plugin.platform,
          arch: plugin.arch,
          node: plugin.node,
          startedAt: plugin.startedAt,
          control: { host: plugin.host, port: plugin.port },
          dshHome: plugin.dshHome,
        })
        return
      }
      default: {
        jsonError(response, 404, 'not found')
      }
    }
  }
}

interface PluginRuntime {
  name: string
  version?: string
  pid: number
  platform: string
  arch: string
  node: string
  startedAt: string
  host: string
  port: number
  dshHome: string
}

function defaultDshHome(): string {
  if (process.env.DSH_HOME !== undefined && process.env.DSH_HOME !== '') {
    return process.env.DSH_HOME
  }
  return join(homedir(), '.dsh')
}

/**
 * Cordis plugin entry. Runs inside the booted harness tree and opens the
 * control channel described at the top of this file.
 */
export function apply(ctx: PluginContext = {}, config: BridgeConfig = {}): void {
  const host = config.host ?? '127.0.0.1'
  const requestedPort = typeof config.port === 'number' ? config.port : 0

  const runtime: PluginRuntime = {
    name,
    version: installedVersion(),
    pid: process.pid,
    platform: process.platform,
    arch: process.arch,
    node: process.version,
    startedAt: new Date().toISOString(),
    host,
    port: 0, // replaced below once the kernel assigns one
    dshHome: defaultDshHome(),
  }

  const server = http.createServer(handleRequest(runtime))

  // Announce the control endpoint on stdout once listening. The desktop
  // client scans the harness stdout stream for this exact prefix.
  server.on('listening', () => {
    const address = server.address()
    const actualPort = typeof address === 'object' && address !== null ? address.port : 0
    runtime.port = actualPort
    console.log(`dsh-desktop-ready ${JSON.stringify({ port: actualPort, host, version: installedVersion() })}`)
  })

  server.on('error', (error: Error) => {
    // Surface bind failures through the harness log; the client falls back
    // to the web app URL without the control channel.
    console.error(`dsh-desktop-bridge: control server failed: ${String(error)}`)
  })

  server.listen(requestedPort, host)

  const closeServer = (): void => {
    try {
      server.close()
    } catch {
      // already closed or never fully started
    }
  }
  // Cordis scope disposal (graceful drain / HMR) and process exit both close
  // the server so the port never leaks to the next run.
  try {
    ctx.on?.('dispose', closeServer)
  } catch {
    // ctx shape varies across harness versions; process exit still cleans up
  }
  process.once('exit', closeServer)
}
