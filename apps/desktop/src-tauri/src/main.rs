//! dsh desktop — Tauri 2 client shell for DeepSeek Harness (dsh).
//!
//! This project never vendors dsh source code. The shell's startup contract:
//!
//!  1. Check whether `dsh` exists on the system. When it is missing, install
//!     it through the official npm channel (`npm install -g @deepseek-ai/dsh`).
//!  2. Ensure the `desktop` profile exists under `$DSH_HOME/profiles/desktop`
//!     and is composed of the official bundles plus this project's own plugin:
//!     `@deepseek-ai/dsh-base` → `@deepseek-ai/dsh-web-app` →
//!     `dsh-desktop-bridge`. Composition uses the official `dsh plugin`
//!     command; no profile files are hand-written.
//!  3. Boot `dsh --profile desktop --no-open`. Communication with the running
//!     harness is plugin-based: the `dsh-desktop-bridge` plugin announces a
//!     loopback control API on stdout (`dsh-desktop-ready {json}`), and the
//!     shell verifies that channel over HTTP. The web UI itself is dsh's own
//!     web app, opened at the authenticated URL the harness prints
//!     (`dsh web: http://127.0.0.1:PORT/?token=...`).
//!  4. When the window closes, the backend gets a graceful SIGTERM (5 s drain,
//!     then SIGKILL on unix; SIGKILL elsewhere).

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const PROFILE_NAME: &str = "desktop";
const DSH_NPM_PACKAGE: &str = "@deepseek-ai/dsh";
const WEB_APP_BUNDLE: &str = "@deepseek-ai/dsh-web-app";
const BRIDGE_PACKAGE_DEFAULT: &str = "dsh-desktop-bridge";
/// Env override for the bridge install spec (a published name or a path to a
/// packed tarball), used in development until the package ships on npm.
const BRIDGE_PACKAGE_ENV: &str = "DSH_DESKTOP_BRIDGE_PACKAGE";
/// Env override pointing the shell at a specific dsh binary/path.
const DSH_BIN_ENV: &str = "DSH_DESKTOP_DSH";

/// Port of the web app served by the dsh web profile (its default).
const WEB_PORT: u16 = 3080;
const WEB_URL: &str = "http://127.0.0.1:3080";

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const READY_TIMEOUT: Duration = Duration::from_secs(240);
const BRIDGE_WAIT: Duration = Duration::from_secs(10);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(480);
const PROFILE_ADD_TIMEOUT: Duration = Duration::from_secs(300);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(6);

/// The spawned harness child, killed (gracefully) when the app exits.
struct ManagedBackend(Mutex<Option<Child>>);

/// Lines the stdout reader watches for. `auth_url` gates the window
/// navigation; `bridge_line` locates the plugin control API.
#[derive(Default)]
struct Markers {
    auth_url: Mutex<Option<String>>,
    bridge_line: Mutex<Option<Value>>,
}

// ── small helpers ───────────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".dsh-desktop")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn dsh_home() -> PathBuf {
    match env::var_os("DSH_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => home_dir().join(".dsh"),
    }
}

/// Append one line to a log file under the app data dir (creates the dir).
fn app_log(line: &str) {
    let _ = fs::create_dir_all(data_dir());
    let path = data_dir().join("dsh-desktop.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", line);
    }
}

/// Push a status update into the bootstrap page shown before the web UI.
fn phase(win: &WebviewWindow, kind: &str, status: &str, detail: &str) {
    let k = serde_json::to_string(kind).unwrap_or_else(|_| "\"info\"".into());
    let s = serde_json::to_string(status).unwrap_or_else(|_| "\"\"".into());
    let d = serde_json::to_string(detail).unwrap_or_else(|_| "\"\"".into());
    let _ = win.eval(&format!("window.__dsd && window.__dsd.show({k}, {s}, {d})"));
    app_log(&format!("[phase:{kind}] {status} — {detail}"));
}

// ── process plumbing ────────────────────────────────────────────────────────

/// Scan one output line for the two machine markers the shell depends on.
fn scan_markers(line: &str, markers: &Markers) {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("dsh web: ") {
        // `dsh web: http://127.0.0.1:3080/?token=... (LAN: ...)` — the local
        // authenticated URL is the first token.
        if let Some(candidate) = rest.split_whitespace().next() {
            if candidate.starts_with("http://127.0.0.1:") && candidate.contains("?token=") {
                *markers.auth_url.lock().unwrap() = Some(candidate.to_string());
            }
        }
    } else if let Some(rest) = trimmed.strip_prefix("dsh-desktop-ready ") {
        if let Ok(value) = serde_json::from_str::<Value>(rest) {
            *markers.bridge_line.lock().unwrap() = Some(value);
        }
    }
}

/// Copy a child output stream to a log file, scanning for markers on stdout.
fn tee_stream(stream: impl Read + Send + 'static, log: PathBuf, markers: Option<Arc<Markers>>) {
    std::thread::spawn(move || {
        let mut file = OpenOptions::new().create(true).append(true).open(&log).ok();
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(m) = &markers {
                scan_markers(&line, m);
            }
            if let Some(f) = &mut file {
                let _ = writeln!(f, "{line}");
            }
        }
    });
}

/// Spawn a command with stdout/stderr piped to a shared log file.
fn spawn_teed(cmd: &mut Command, log: &Path) -> std::io::Result<Child> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    if let Some(out) = child.stdout.take() {
        tee_stream(out, log.to_path_buf(), None);
    }
    if let Some(err) = child.stderr.take() {
        tee_stream(err, log.to_path_buf(), None);
    }
    Ok(child)
}

/// Wait for a child with a timeout; on timeout SIGKILL it. Returns
/// (timed_out, exit_code).
fn wait_with_timeout(mut child: Child, timeout: Duration, label: &str) -> (bool, Option<i32>) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                app_log(&format!("{label}: exited with {status}"));
                return (false, status.code());
            }
            Ok(None) => {}
            Err(e) => {
                app_log(&format!("{label}: wait error: {e}"));
                return (false, None);
            }
        }
        if start.elapsed() >= timeout {
            app_log(&format!("{label}: timed out after {timeout:?}, killing"));
            let _ = child.kill();
            let _ = child.wait();
            return (true, None);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Stop the backend: graceful SIGTERM, then SIGKILL after the drain window.
#[cfg(unix)]
fn terminate(child: &mut Child) {
    let pid = child.id();
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    app_log("backend did not drain within the grace window — SIGKILL");
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// Minimal HTTP GET over a raw socket (loopback only) returning parsed JSON.
fn http_get_json(port: u16, path: &str) -> Option<Value> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(600)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 262_144 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let head = text.lines().next().unwrap_or("");
    if !(head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200")) {
        return None;
    }
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    serde_json::from_str(body.trim()).ok()
}

// ── the three startup phases ────────────────────────────────────────────────

/// Probe for a usable `dsh` on the system (env override wins).
fn probe_dsh() -> Option<(String, String)> {
    let candidates: Vec<String> = match env::var(DSH_BIN_ENV) {
        Ok(custom) if !custom.trim().is_empty() => vec![custom.trim().to_string()],
        _ => vec!["dsh".to_string()],
    };
    for bin in candidates {
        if let Ok(out) = Command::new(&bin).arg("--version").output() {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return Some((bin, version));
            }
        }
    }
    None
}

/// Phase 1 — install dsh through the official npm channel when missing.
fn install_dsh(win: &WebviewWindow) -> Result<String, String> {
    let _ = fs::create_dir_all(data_dir());
    let log = data_dir().join("install.log");
    phase(win, "info", "Installing dsh…", "dsh was not found — running the official install (npm install -g @deepseek-ai/dsh). This can take a few minutes.");
    app_log(&format!("npm install -g {DSH_NPM_PACKAGE}"));
    let mut cmd = Command::new("npm");
    cmd.args(["install", "-g", DSH_NPM_PACKAGE]);
    let child = spawn_teed(&mut cmd, &log)
        .map_err(|e| format!("Could not run npm ({e}). Install Node.js first (https://nodejs.org), then relaunch dsh desktop."))?;
    let (timed_out, code) = wait_with_timeout(child, INSTALL_TIMEOUT, "npm install -g dsh");
    if timed_out {
        return Err("Installing dsh timed out. Check the network and relaunch.".into());
    }
    if code != Some(0) {
        return Err(format!(
            "npm failed to install dsh (exit {code:?}). See {}",
            log.display()
        ));
    }
    probe_dsh().map(|(bin, _)| bin).ok_or_else(|| {
        "npm reported success but dsh is still not on PATH — restart dsh desktop.".into()
    })
}

/// The bridge install spec: a published package name by default, overridable
/// for development (e.g. a path to a packed tarball).
fn bridge_package() -> String {
    env::var(BRIDGE_PACKAGE_ENV).unwrap_or_else(|_| BRIDGE_PACKAGE_DEFAULT.to_string())
}

/// Locate a bundle that ships inside the installed dsh itself (in-box
/// bundles like the web app), so the profile can link it directly instead of
/// re-fetching from a registry. Returns a pnpm `file:` spec when found.
fn installed_bundle_spec(pkg: &str) -> Option<String> {
    let root = Command::new("npm").args(["root", "-g"]).output().ok()?;
    if !root.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&root.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let top = PathBuf::from(&root).join("@deepseek-ai").join(pkg);
    let nested = PathBuf::from(&root)
        .join("@deepseek-ai")
        .join("dsh")
        .join("node_modules")
        .join("@deepseek-ai")
        .join(pkg);
    for dir in [top, nested] {
        if dir.join("package.json").exists() {
            return Some(format!("file:{}", dir.display()));
        }
    }
    None
}

/// Install specs to try for a missing bundle, most desirable first. The web
/// app links the copy inside the dsh installation (guaranteed same version,
/// no registry round-trip); the bridge uses the configured spec.
fn candidate_specs(kind: &str) -> Vec<String> {
    match kind {
        "web-app" => vec![
            installed_bundle_spec("dsh-web-app").unwrap_or_else(|| WEB_APP_BUNDLE.to_string()),
            WEB_APP_BUNDLE.to_string(),
        ],
        _ => vec![bridge_package()],
    }
}

/// Phase 2 — compose the `desktop` profile via the official `dsh plugin`
/// command (never hand-written profile files).
fn ensure_desktop_profile(bin: &str, win: &WebviewWindow) -> Result<(), String> {
    let manifest = dsh_home()
        .join("profiles")
        .join(PROFILE_NAME)
        .join("package.json");
    let mut missing: Vec<String> = Vec::new();
    if let Ok(text) = fs::read_to_string(&manifest) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            let installed = value
                .pointer("/dsh/profile/bundles")
                .and_then(|b| b.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !installed.contains(&WEB_APP_BUNDLE.to_string()) {
                missing.push("web-app".to_string());
            }
            let bridge = bridge_package();
            if !installed.contains(&bridge) {
                missing.push("bridge".to_string());
            }
        }
    } else {
        // Fresh profile: dsh plugin's first add initializes it, then both
        // bundle dependencies are added in order.
        missing.push("web-app".to_string());
        missing.push("bridge".to_string());
    }
    if missing.is_empty() {
        return Ok(());
    }
    for kind in missing {
        let mut last_error: Option<String> = None;
        for spec in candidate_specs(&kind) {
            phase(
                win,
                "info",
                "Composing the desktop profile…",
                &format!("dsh plugin --profile {PROFILE_NAME} add {spec}"),
            );
            let log = data_dir().join("profile-add.log");
            app_log(&format!("dsh plugin --profile {PROFILE_NAME} add {spec}"));
            let mut cmd = Command::new(bin);
            cmd.args(["plugin", "--profile", PROFILE_NAME, "add", &spec]);
            let child =
                spawn_teed(&mut cmd, &log).map_err(|e| format!("Failed to run dsh plugin: {e}"))?;
            let (timed_out, code) = wait_with_timeout(
                child,
                PROFILE_ADD_TIMEOUT,
                &format!("dsh plugin add {spec}"),
            );
            if timed_out {
                last_error = Some(
                    "Composing the desktop profile timed out (network?). Relaunch to retry.".into(),
                );
                continue;
            }
            if code != Some(0) {
                last_error = Some(format!(
                    "dsh plugin add {spec} failed (exit {code:?}). See {}",
                    log.display()
                ));
                continue;
            }
            last_error = None;
            break;
        }
        if let Some(message) = last_error {
            if kind == "bridge" && bridge_package() == BRIDGE_PACKAGE_DEFAULT {
                return Err(format!("{message} — until dsh-desktop-bridge is published, set DSH_DESKTOP_BRIDGE_PACKAGE to a packed tarball (pnpm --filter dsh-desktop-bridge pack) and relaunch."));
            }
            return Err(message);
        }
    }
    Ok(())
}

/// Spawn `dsh --profile desktop --no-open` and return the child plus markers.
fn spawn_backend(bin: &str) -> std::io::Result<(Child, Arc<Markers>)> {
    let _ = fs::create_dir_all(data_dir());
    let log = data_dir().join("backend.log");
    let mut cmd = Command::new(bin);
    cmd.args(["--profile", PROFILE_NAME, "--no-open"]);
    // Predictable initial workspace root; the real one is picked in the web UI.
    cmd.current_dir(home_dir());
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let markers = Arc::new(Markers::default());
    if let Some(out) = child.stdout.take() {
        tee_stream(out, log.clone(), Some(markers.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tee_stream(err, log, None);
    }
    Ok((child, markers))
}

/// Phase 3 — wait for the authenticated web URL the backend announces.
fn wait_for_web(markers: &Markers) -> Option<String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        let url = markers.auth_url.lock().unwrap().clone();
        if let Some(url) = url {
            return Some(url);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    None
}

fn verify_bridge(win: &WebviewWindow, markers: &Markers) {
    // The bridge announcement may land slightly after the web URL.
    let mut line = markers.bridge_line.lock().unwrap().clone();
    let deadline = Instant::now() + BRIDGE_WAIT;
    while line.is_none() && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        line = markers.bridge_line.lock().unwrap().clone();
    }
    let Some(line) = line else {
        app_log("no dsh-desktop-ready line — plugin control channel not present");
        return;
    };
    let port = line.get("port").and_then(Value::as_u64).unwrap_or(0) as u16;
    if port == 0 {
        app_log("dsh-desktop-ready announced no port — control channel not usable");
        return;
    }
    // Poll the plugin's health endpoint to prove the client↔dsh channel.
    for _ in 0..25 {
        if let Some(health) = http_get_json(port, "/api/desktop/health") {
            let info = http_get_json(port, "/api/desktop/info").unwrap_or_default();
            app_log(&format!(
                "bridge control channel OK on port {port}: {health} {info}"
            ));
            phase(
                win,
                "ok",
                "Connected",
                "dsh desktop control channel is live via dsh-desktop-bridge.",
            );
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    app_log(&format!(
        "bridge announced port {port} but health never answered"
    ));
}

fn navigate_to(handle: &tauri::AppHandle, target: &str) {
    let target = target.to_string();
    let for_thread = handle.clone();
    let _ = handle.run_on_main_thread(move || {
        if let Some(win) = for_thread.get_webview_window("main") {
            if let Ok(url) = tauri::Url::parse(&target) {
                let _ = win.navigate(url);
            }
        }
    });
}

/// The whole startup contract, run off the main thread so the UI stays live.
fn run_bootstrap(handle: tauri::AppHandle, win: WebviewWindow) {
    // Already-serving backend? Reuse it instead of spawning a second one that
    // would fail to bind and leave the window staring at an error.
    if port_open(WEB_PORT) {
        app_log("port 3080 already serves a dsh web backend — reusing it");
        phase(
            &win,
            "ok",
            "Connecting…",
            "Reusing an existing dsh web backend.",
        );
        std::thread::sleep(Duration::from_millis(800));
        navigate_to(&handle, WEB_URL);
        return;
    }

    phase(
        &win,
        "info",
        "Checking for dsh…",
        "Looking for the DeepSeek Harness CLI on PATH.",
    );

    let bin = match probe_dsh() {
        Some((bin, version)) => {
            app_log(&format!("dsh found: {bin} ({version})"));
            bin
        }
        None => match install_dsh(&win) {
            Ok(bin) => bin,
            Err(message) => {
                phase(&win, "error", "Could not start dsh desktop", &message);
                return;
            }
        },
    };

    phase(
        &win,
        "info",
        "Preparing the desktop profile…",
        "Ensuring $DSH_HOME/profiles/desktop exists with the dsh desktop plugin.",
    );
    if let Err(message) = ensure_desktop_profile(&bin, &win) {
        phase(
            &win,
            "error",
            "Could not prepare the desktop profile",
            &message,
        );
        return;
    }

    phase(
        &win,
        "info",
        "Starting DeepSeek Harness…",
        "Booting dsh with the desktop profile.",
    );
    let (child, markers) = match spawn_backend(&bin) {
        Ok(pair) => pair,
        Err(e) => {
            phase(
                &win,
                "error",
                "Could not start the dsh backend",
                &format!("{e}"),
            );
            return;
        }
    };
    *handle.state::<ManagedBackend>().0.lock().unwrap() = Some(child);

    match wait_for_web(&markers) {
        Some(url) => {
            app_log(&format!("authenticated web URL ready: {url}"));
            phase(&win, "ok", "Launching dsh desktop…", "");
            navigate_to(&handle, &url);
            // Give the page a beat, then prove the plugin control channel.
            std::thread::sleep(Duration::from_secs(2));
            verify_bridge(&win, &markers);
        }
        None => {
            // Did the backend die on its own, or are we just past the clock?
            let exited = {
                let state = handle.state::<ManagedBackend>();
                let mut guard = match state.0.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                match guard.as_mut() {
                    Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                    None => false,
                }
            };
            if exited {
                phase(
                    &win,
                    "error",
                    "The dsh backend exited during startup",
                    "See ~/.dsh-desktop/backend.log for details.",
                );
            } else {
                phase(
                    &win,
                    "error",
                    "dsh did not become ready in time",
                    "See ~/.dsh-desktop/backend.log for details.",
                );
            }
        }
    }
}

// ── app entry ───────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .manage(ManagedBackend(Mutex::new(None)))
        .setup(|app| {
            let handle = app.handle().clone();
            let win =
                WebviewWindowBuilder::new(&handle, "main", WebviewUrl::App("index.html".into()))
                    .title("dsh desktop")
                    .inner_size(1280.0, 840.0)
                    .min_inner_size(860.0, 560.0)
                    .build()?;
            std::thread::spawn(move || run_bootstrap(handle, win));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the dsh desktop application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(mut child) = app
                    .state::<ManagedBackend>()
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut c| c.take())
                {
                    app_log("window closed — stopping the dsh backend");
                    terminate(&mut child);
                }
            }
        });
}
