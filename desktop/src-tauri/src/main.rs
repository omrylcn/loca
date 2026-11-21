// Loca desktop — thin Tauri wrapper.
//
// It bundles and loads the SAME web UI (../../web) and lets that UI talk to a
// configurable loca-server over WS+REST, exactly like the browser client. There
// is no forked UI and no forked backend here.
//
// Phase 1: window shell + the server-URL shim (SERVER_SHIM).
// Phase 2 (this file): move the two credential-bearing localStorage keys off the
//   webview's on-disk store into the OS keychain, WITHOUT forking the web UI.
//
//   The web UI (measured in web/assets/) persists exactly two sensitive keys:
//     - "loca-seat"          => { name, roomToken (=invite), room }   (api.js)
//     - "loca-admin-session" => { token, expiresAt }                 (session.js)
//   Everything else in localStorage is non-secret (preferences, read cursors,
//   lobby height) and stays in plain localStorage untouched.
//
//   The web UI reads the seat SYNCHRONOUSLY at boot (app.js: JSON.parse(
//   localStorage.getItem("loca-seat"))). Keychain reads are async, so an
//   async-hydrate shim would race that boot read. Race-free design instead:
//   Rust reads the keychain SYNCHRONOUSLY at window-build (keyring is sync) and
//   injects the values into the initialization script, so the JS Storage proxy
//   can answer getItem() synchronously from first paint. Writes go back to the
//   keychain via the kc_set/kc_delete commands (async, for next launch); the
//   plaintext never reaches the on-disk localStorage.
//
// BUILD NOTE for the maintainer: this targets the Tauri v2 builder API
// (WebviewWindowBuilder + initialization_script) and keyring v3. Verify the
// exact keyring feature flags for the Secret Service backend at build time.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::Manager;
use tauri::{WebviewUrl, WebviewWindowBuilder}; // app.path()/app.manage()

const ZOOM_LEVELS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
const DEFAULT_ZOOM_INDEX: usize = 5;
const ZOOM_IN_ACCELERATOR: &str = "CmdOrCtrl+=";
const ZOOM_OUT_ACCELERATOR: &str = "CmdOrCtrl+-";
const ZOOM_RESET_ACCELERATOR: &str = "CmdOrCtrl+0";
static ZOOM_INDEX: AtomicUsize = AtomicUsize::new(DEFAULT_ZOOM_INDEX);

fn next_zoom_index(current: usize, direction: i8) -> usize {
    match direction.cmp(&0) {
        std::cmp::Ordering::Greater => (current + 1).min(ZOOM_LEVELS.len() - 1),
        std::cmp::Ordering::Less => current.saturating_sub(1),
        std::cmp::Ordering::Equal => DEFAULT_ZOOM_INDEX,
    }
}

fn apply_zoom(app: &tauri::AppHandle, direction: i8) {
    let next = next_zoom_index(ZOOM_INDEX.load(Ordering::Relaxed), direction);
    ZOOM_INDEX.store(next, Ordering::Relaxed);
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.set_zoom(ZOOM_LEVELS[next]) {
            eprintln!("failed to set desktop zoom: {error}");
        }
    }
}

// Keychain service namespace + the exact credential keys mirrored from the web
// UI. Keep this list in sync with the localStorage keys above; any key NOT here
// is left in plain localStorage by the JS shim.
//
// The standalone flavor uses a SEPARATE namespace so its auto-provisioned local
// seat never collides with the client flavor's hosted seat on the same machine
// (a shared "loca-seat" would make standalone skip provisioning and hand the
// local server a foreign invite).
#[cfg(not(feature = "bundled-server"))]
const KC_SERVICE: &str = "tech.speakbetter.loca.desktop";
#[cfg(feature = "bundled-server")]
const KC_SERVICE: &str = "tech.speakbetter.loca.desktop.standalone";
const KC_KEYS: [&str; 2] = ["loca-seat", "loca-admin-session"];

// Baked at build time so the public desktop client opens ALREADY pointed at a
// server instead of an empty box (the "it has no charm" complaint):
// LOCA_DEFAULT_SERVER=https://loca.speakbetter.tech (the shared hosted
// server). Unset -> empty -> the field just prompts (current dev behaviour).
// The user can always override via the #server field (persisted as
// loca-desktop-server). The Host never uses it: its page talks only to the
// server it started (standalone::page_server), so a baked address such as
// http://127.0.0.1:8787 would hand the Host UI and its credentials to
// whatever listens there.
#[cfg(not(feature = "bundled-server"))]
const DEFAULT_SERVER: &str = match option_env!("LOCA_DEFAULT_SERVER") {
    Some(s) => s,
    None => "",
};

// The one origin this window is built to show: the app's own bundled assets,
// served by Tauri's custom protocol, tauri://localhost, or on Windows its
// http://tauri.localhost stand-in (tauri::manager tauri_protocol_url; this app
// sets no useHttpsScheme). Neither flavor shows anything else at the top
// level: the window opens WebviewUrl::default(), the servers (the Host's own,
// the client's chosen one) are reached by fetch and WebSocket, and the page's
// links leave through target="_blank". The keychain hand-over is given only
// to this origin (KEYCHAIN_SHIM) and the window is not let navigate away
// from it (may_navigate).
const APP_ORIGIN: &str = if cfg!(windows) {
    "http://tauri.localhost"
} else {
    "tauri://localhost"
};

// scheme://host[:port] of a URL, as location.origin spells it for the app's
// own origin (url::Url::origin() is opaque for the custom tauri scheme).
fn url_origin(url: &tauri::Url) -> String {
    let host = url.host_str().unwrap_or("");
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

// A top-level navigation stays in the app's own origin, or is refused. A blob:
// URL the page itself made (blob:<app origin>/<uuid>) is the app's own: the
// web UI saves and opens attachments through such URLs (chat.js
// attachmentChip, openAttachment), and some engines ask this handler about
// them. The server serves attachments only as png, jpeg, webp, pdf and
// text/plain, so such a document runs no script.
fn may_navigate(url: &tauri::Url) -> bool {
    if url.scheme() == "blob" {
        return tauri::Url::parse(url.path())
            .map(|inner| url_origin(&inner) == APP_ORIGIN)
            .unwrap_or(false);
    }
    url_origin(url) == APP_ORIGIN
}

// What the window may save to disk: only a file the page itself made
// (blob:<app origin>/<uuid>), which is how the web UI hands over an attachment
// it fetched with the session (chat.js attachmentChip). Nothing else: no
// address of another origin, no data: URL. Without a download handler
// WebKitGTK saved nothing at all; with one, wry puts the file in the
// person's Downloads folder under a name that overwrites nothing.
fn may_download(url: &tauri::Url) -> bool {
    url.scheme() == "blob" && may_navigate(url)
}

// The page is told where the file went (or that it was not saved): the
// window has no download bar of its own.
fn download_notice(path: Option<&std::path::Path>, success: bool) -> String {
    match (success, path) {
        (true, Some(path)) => format!("saved to {}", path.display()),
        (true, None) => "saved to your Downloads folder".to_string(),
        (false, _) => "the download did not finish".to_string(),
    }
}

// Where a download goes: the person's Downloads folder (XDG), or
// ~/Downloads when the system names none. Never the process's working
// directory, which is wry's own fallback: on a session without XDG user dirs
// a desktop check saved its files into the directory the app was started
// from.
fn download_dir(
    named: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    named.or_else(|| home.map(|home| home.join("Downloads")))
}

// `name` in `dir`, or "name (1).ext", "name (2).ext", ... when taken, the way
// wry and the browsers number a second copy: nothing is overwritten.
fn unused_path(dir: &std::path::Path, name: &std::ffi::OsStr) -> std::path::PathBuf {
    let name = name.to_string_lossy();
    let (base, ext) = match name.split_once('.') {
        Some((base, ext)) if !base.is_empty() => (base.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    let mut path = dir.join(&*name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{base} ({n}){ext}"));
        n += 1;
    }
    path
}

fn on_download<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    event: tauri::webview::DownloadEvent<'_>,
) -> bool {
    use tauri::webview::DownloadEvent;
    match event {
        DownloadEvent::Requested { url, destination } => {
            if !may_download(&url) {
                return false;
            }
            let Some(dir) = download_dir(
                webview.path().download_dir().ok(),
                webview.path().home_dir().ok(),
            ) else {
                return false;
            };
            if std::fs::create_dir_all(&dir).is_err() {
                return false;
            }
            let name = destination
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_else(|| "attachment".into());
            *destination = unused_path(&dir, &name);
            true
        }
        DownloadEvent::Finished { url, path, success } => {
            if may_download(&url) {
                let notice = download_notice(path.as_deref(), success);
                if let Ok(text) = serde_json::to_string(&notice) {
                    let _ =
                        webview.eval(format!("typeof addSys === \"function\" && addSys({text});"));
                }
            }
            true
        }
        _ => true,
    }
}

fn kc_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KC_SERVICE, key).map_err(|e| e.to_string())
}

// Read one secret; absent -> Ok(None) (NoEntry is not an error).
fn kc_read(key: &str) -> Result<Option<String>, String> {
    match kc_entry(key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn kc_set(key: String, value: String) -> Result<(), String> {
    // Only ever store the known credential keys — never let the webview push
    // arbitrary keys into the OS keychain.
    if !KC_KEYS.contains(&key.as_str()) {
        return Err(format!("refusing to store unknown key: {key}"));
    }
    kc_entry(&key)?
        .set_password(&value)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn kc_get(key: String) -> Result<Option<String>, String> {
    if !KC_KEYS.contains(&key.as_str()) {
        return Ok(None);
    }
    kc_read(&key)
}

#[tauri::command]
fn kc_delete(key: String) -> Result<(), String> {
    if !KC_KEYS.contains(&key.as_str()) {
        return Ok(());
    }
    match kc_entry(&key)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ── Phase 3: native notifications ─────────────────────────────────────────────
// Fired from JS via invoke("notify", {kind, sender, loca, msgId}). PRIVACY: the
// notification shows only WHO and WHAT KIND of event — never the message body,
// so a lock screen never leaks room text. (loca, msgId) ride along only so a
// future click can route to the right place; they are not shown.
#[derive(serde::Deserialize)]
struct NotifyPayload {
    kind: String, // "mention" | "attention" | "reminder"
    sender: Option<String>,
    // Carried for click-routing (not yet consumed); kept so the JS contract is
    // stable and never displayed.
    #[allow(dead_code)]
    loca: Option<String>,
    // The event's unique id (message id or attention id), used JS-side for
    // dedup and (later) click-routing. Never displayed.
    #[allow(dead_code)]
    id: Option<String>,
}

#[tauri::command]
fn notify(app: tauri::AppHandle, payload: NotifyPayload) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let who = payload.sender.as_deref().unwrap_or("someone");
    // Whitelist: only the three notifiable kinds reach the OS layer; anything
    // else is rejected (defense against a bad/forged payload). Title = who +
    // event kind only — no body, so a lock screen never shows room text.
    let title = match payload.kind.as_str() {
        "mention" => format!("{who} mentioned you"),
        "reminder" => "Reminder".to_string(),
        "attention" => format!("{who} needs your attention"),
        other => return Err(format!("unknown notification kind: {other}")),
    };
    app.notification()
        .builder()
        .title(title)
        .show()
        .map_err(|e| e.to_string())
}

// The one window the shell builds, and the only one the Host's Master session
// is given to (host_master_session).
const MAIN_WINDOW: &str = "main";

fn is_host_window(label: &str) -> bool {
    label == MAIN_WINDOW
}

// Loca Host only: a new Master session for the Host's own page when the one it
// was given has ended (web/assets/session.js hostMasterSession). It is the start
// provisioning again (standalone::renew_master_session): the page gets the
// bounded session, never the admin token. Only the window this app built
// can invoke it; the client flavor has no Host to ask and refuses.
//
// Tauri offers an app's commands to every window and webview of the app. The
// shell builds one window today (MAIN_WINDOW); the command says so itself, so
// a window added later does not get the Host's Master session by default.
#[tauri::command]
async fn host_master_session(window: tauri::WebviewWindow) -> Result<String, String> {
    if !is_host_window(window.label()) {
        return Err("only Loca Host's own window renews its Master session".to_string());
    }
    #[cfg(feature = "bundled-server")]
    {
        tauri::async_runtime::spawn_blocking(standalone::renew_master_session)
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(feature = "bundled-server"))]
    {
        Err("only Loca Host renews its own Master session".to_string())
    }
}

// ── Option 3 (standalone flavor): bundled room-server sidecar ────────────────
// Only compiled with `--features bundled-server`. Boots a LOCAL, CLOSED-door
// room-server on 127.0.0.1 at launch, provisions the user's own seat, and points
// the UI there — so the app has no dependency on an external server. The whole
// HTTP flow below was verified against a real closed-door room-server before it
// was written (admit -> invite -> session, HTTP 201).
#[cfg(feature = "bundled-server")]
mod standalone;

// Framework-agnostic JS, desktop-only. Runs at document-start (before the web
// UI's own scripts), so it must NOT touch the #server field here — that element
// doesn't exist yet, and the shared app.js reads window.__LOCA_DEFAULT_SERVER__
// synchronously during its own startup (app.js: `... || location.origin`). To
// avoid the earlier race (setting the field on window.load, after app.js already
// called doConnect/refreshRooms), we only (a) fold a saved user override INTO
// that global before app.js reads it, and (b) persist future manual changes.
const SERVER_SHIM: &str = include_str!("shims/server.js");

// Storage proxy: for the credential keys, serve reads from the Rust-injected
// boot cache and route writes to the OS keychain; all other keys pass straight
// through to real localStorage. Runs at document-start, before the web UI's
// scripts, so the override is in place before the first getItem("loca-seat").
const KEYCHAIN_SHIM: &str = include_str!("shims/keychain.js");

// Bridges the web UI's notifyDesktop() calls to native OS notifications. Runs at
// document-start so window.__LOCA_NOTIFY__ exists before the first WS frame.
const NOTIFY_SHIM: &str = include_str!("shims/notify.js");

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .menu(|app| {
            use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

            // Tauri v2 does not create the native application menu for us.
            // Start from its standard menu so macOS receives the expected Edit
            // commands (Undo/Redo/Cut/Copy/Paste/Select All), then extend View.
            let menu = Menu::default(app)?;
            let zoom_in = MenuItem::with_id(
                app,
                "zoom_in",
                "Zoom In",
                true,
                Some(ZOOM_IN_ACCELERATOR),
            )?;
            let zoom_out = MenuItem::with_id(
                app,
                "zoom_out",
                "Zoom Out",
                true,
                Some(ZOOM_OUT_ACCELERATOR),
            )?;
            let zoom_reset = MenuItem::with_id(
                app,
                "zoom_reset",
                "Actual Size",
                true,
                Some(ZOOM_RESET_ACCELERATOR),
            )?;
            let separator = PredefinedMenuItem::separator(app)?;

            let mut view_menu_found = false;
            for item in menu.items()? {
                if let Some(submenu) = item.as_submenu() {
                    if submenu.text()? == "View" {
                        submenu.append_items(&[
                            &separator,
                            &zoom_in,
                            &zoom_out,
                            &zoom_reset,
                        ])?;
                        view_menu_found = true;
                        break;
                    }
                }
            }
            if !view_menu_found {
                menu.append(&Submenu::with_items(
                    app,
                    "View",
                    true,
                    &[&zoom_in, &zoom_out, &zoom_reset],
                )?)?;
            }

            // Menu::default leaves Help empty on macOS because About normally
            // lives only under the application-name menu. Keep that native
            // placement, but also make Help useful and give the operator an
            // obvious, platform-independent place to find the packaged version.
            for item in menu.items()? {
                if let Some(submenu) = item.as_submenu() {
                    if submenu.text()? == "Help" {
                        let package = app.package_info();
                        submenu.append(&PredefinedMenuItem::about(
                            app,
                            Some("About Loca"),
                            Some(AboutMetadata {
                                name: Some("Loca".to_string()),
                                version: Some(package.version.to_string()),
                                ..Default::default()
                            }),
                        )?)?;
                        break;
                    }
                }
            }
            Ok(menu)
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "zoom_in" => apply_zoom(app, 1),
            "zoom_out" => apply_zoom(app, -1),
            "zoom_reset" => apply_zoom(app, 0),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            kc_set,
            kc_get,
            kc_delete,
            notify,
            host_master_session
        ])
        .setup(|app| {
            // Standalone flavor: boot the local room-server BEFORE the window,
            // wait for it, and provision the user's seat so the UI connects to a
            // ready, closed-door server. Any failure here degrades gracefully — the
            // UI just shows a connect prompt rather than the app dying.
            #[cfg(feature = "bundled-server")]
            let (standalone_url, mut host_provisioning, fresh_admin_session): (Option<String>, serde_json::Value, Option<String>) = match standalone::spawn(app) {
                Ok((mut child, base)) => {
                    // Check readiness (and child liveness) BEFORE moving the child
                    // into managed state.
                    if standalone::wait_ready(&base, &mut child) {
                        match standalone::provision_if_needed(&base) {
                            Ok(session) => {
                                app.manage(standalone::ServerProc {
                                    child: std::sync::Mutex::new(Some(child)),
                                });
                                standalone::supervise(app.handle().clone());
                                (Some(base), serde_json::json!({ "state": "ready" }), Some(session))
                            }
                            Err(e) => {
                                // The detailed error may contain transport or OS internals. Keep
                                // it in the native log; the webview receives only a stable,
                                // credential-free state and recovery instruction.
                                eprintln!("standalone provisioning failed: {e}");
                                app.manage(standalone::ServerProc {
                                    child: std::sync::Mutex::new(Some(child)),
                                });
                                (Some(base), serde_json::json!({ "state": "error", "stage": "master_session" }), None)
                            }
                        }
                    } else {
                        eprintln!("standalone server did not become ready (or exited)");
                        app.manage(standalone::ServerProc {
                            child: std::sync::Mutex::new(Some(child)),
                        });
                        (Some(base), serde_json::json!({ "state": "error", "stage": "server_start" }), None)
                    }
                }
                Err(e) => {
                    eprintln!("bundled room-server failed to start: {e}");
                    let stage = if e.starts_with(standalone::KEYCHAIN_FAILED) {
                        "keychain"
                    } else {
                        "server_start"
                    };
                    (None, serde_json::json!({ "state": "error", "stage": stage }), None)
                }
            };
            #[cfg(not(feature = "bundled-server"))]
            let host_provisioning = serde_json::json!({ "state": "not_applicable" });

            // Synchronously pull the credential keys out of the OS keychain and
            // inject them so the JS proxy can answer getItem() with no async race.
            let mut boot = serde_json::Map::new();
            for key in KC_KEYS {
                match kc_read(key) {
                    Ok(Some(v)) => {
                        boot.insert(key.to_string(), serde_json::Value::String(v));
                    }
                    Ok(None) => {}
                    // A missing/locked keychain must not stop the app launching;
                    // the user just re-enters the seat this once.
                    Err(e) => eprintln!("keychain read failed for {key}: {e}"),
                }
            }
            #[cfg(feature = "bundled-server")]
            if let Some(session) = fresh_admin_session {
                // Use the session minted in this exact launch. Do not depend on
                // a write-then-read keychain round trip before opening BUILDING.
                boot.insert(
                    "loca-admin-session".to_string(),
                    serde_json::Value::String(session),
                );
            }
            let boot_json = serde_json::to_string(&serde_json::Value::Object(boot))
                .unwrap_or_else(|_| "{}".to_string());
            // Standalone points the UI at the just-booted local server (dynamic
            // port); if it failed to start, at none (the page then shows the
            // failure and talks to nobody). The client flavor always uses the
            // build-time baked default.
            #[cfg(feature = "bundled-server")]
            let effective_default = standalone::page_server(standalone_url.as_deref());
            #[cfg(not(feature = "bundled-server"))]
            let effective_default = DEFAULT_SERVER;
            let default_server = serde_json::to_string(effective_default)
                .unwrap_or_else(|_| "\"\"".to_string());
            // Standalone is locked to its local sidecar (no remote override); the
            // client flavor is freely re-pointable.
            #[cfg(feature = "bundled-server")]
            let lock_server = "true";
            #[cfg(not(feature = "bundled-server"))]
            let lock_server = "false";
            // The Host's window belongs to the Host's owner, the Master, with or
            // without a Master session yet: the Getting Started guide opens its
            // Host view there at once (web/assets/getstarted.js isHostContext).
            #[cfg(feature = "bundled-server")]
            let is_host = "true";
            #[cfg(not(feature = "bundled-server"))]
            let is_host = "false";
            // Extract the embedded skill library to a versioned, read-only local
            // path so an agent can install a skill straight from disk — no server,
            // no network. On failure we do NOT hide the reason: the error is
            // logged AND surfaced to the guide, which shows a visible "Skill
            // Library unavailable" status and falls back to the download command.
            let app_data = app.path().app_data_dir();
            let (skill_library, skill_library_error): (Option<String>, Option<String>) =
                match &app_data {
                    Ok(data) => match skill_bundles::install_versioned(&data.join("skill-library")) {
                        Ok(dir) => (Some(dir.to_string_lossy().into_owned()), None),
                        Err(e) => {
                            eprintln!("skill library install failed: {e}");
                            (None, Some(e.to_string()))
                        }
                    },
                    Err(e) => {
                        eprintln!("app_data_dir unavailable for skill library: {e}");
                        (None, Some(e.to_string()))
                    }
                };
            #[cfg(feature = "bundled-server")]
            if skill_library_error.is_some() {
                host_provisioning =
                    serde_json::json!({ "state": "error", "stage": "skill_library" });
            }
            #[cfg(feature = "bundled-server")]
            if let (Some(server), Ok(data)) = (standalone_url.as_deref(), app_data.as_ref()) {
                let state = host_provisioning["state"].as_str().unwrap_or("error");
                let stage = host_provisioning["stage"].as_str().unwrap_or("none");
                if let Err(e) = standalone::write_host_env(
                    data,
                    server,
                    skill_library.as_deref(),
                    state,
                    stage,
                ) {
                    eprintln!("host runtime marker failed: {e}");
                    host_provisioning =
                        serde_json::json!({ "state": "error", "stage": "host_env" });
                }
            }
            let skill_library_json =
                serde_json::to_string(&skill_library).unwrap_or_else(|_| "null".to_string());
            let skill_library_error_json =
                serde_json::to_string(&skill_library_error).unwrap_or_else(|_| "null".to_string());
            let host_provisioning_json = serde_json::to_string(&host_provisioning)
                .unwrap_or_else(|_| "{\"state\":\"error\",\"stage\":\"bootstrap\"}".to_string());

            // The keychain values are written into the page only on the app's own
            // origin; the script runs on every top-level page of the window.
            let origins_json =
                serde_json::to_string(&[APP_ORIGIN]).unwrap_or_else(|_| "[]".to_string());
            let init = format!(
                "window.__LOCA_DESKTOP__ = true;\n\
                 window.__LOCA_HOST__ = {is_host};\n\
                 window.__LOCA_LOCK_SERVER__ = {lock_server};\n\
                 window.__LOCA_DEFAULT_SERVER__ = {default_server};\n\
                 window.__LOCA_SKILL_LIBRARY__ = {skill_library_json};\n\
                 window.__LOCA_SKILL_LIBRARY_ERROR__ = {skill_library_error_json};\n\
                 window.__LOCA_HOST_PROVISIONING__ = {host_provisioning_json};\n\
                 (function () {{ var o = {origins_json}; if (o.indexOf(location.origin) === -1) return;\n\
                   window.__LOCA_KC_ORIGINS__ = o; window.__LOCA_KC_BOOT__ = {boot_json}; }})();\n\
                 {KEYCHAIN_SHIM}\n{SERVER_SHIM}\n{NOTIFY_SHIM}"
            );

            WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
                .title("Loca")
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 520.0)
                .initialization_script(&init)
                .on_navigation(may_navigate)
                .on_download(on_download)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Loca desktop");

    // Own run-loop so the standalone flavor can stop its child server on exit.
    app.run(|_app_handle, _event| {
        #[cfg(feature = "bundled-server")]
        if matches!(
            _event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            standalone::stop(_app_handle);
        }
    });
}

#[cfg(test)]
mod admin_token_boundary {
    use super::*;

    #[test]
    fn the_window_saves_only_files_the_page_made() {
        let own_blob = format!("blob:{APP_ORIGIN}/0b1c2d3e-0000-4000-8000-000000000000");
        assert!(may_download(&tauri::Url::parse(&own_blob).unwrap()));
        for other in [
            "blob:https://example.com/0b1c2d3e-0000-4000-8000-000000000000",
            "https://example.com/file.pdf",
            "http://127.0.0.1:8787/rooms/a/attachments/x",
            "data:text/plain,hello",
            "file:///etc/passwd",
        ] {
            assert!(!may_download(&tauri::Url::parse(other).unwrap()), "{other}");
        }
        // The app's own origin is a page, not a file to save.
        assert!(!may_download(&tauri::Url::parse(APP_ORIGIN).unwrap()));
    }

    #[test]
    fn downloads_go_to_the_downloads_folder_never_the_working_directory() {
        use std::path::PathBuf;
        let named = PathBuf::from("/home/you/Hämtningar");
        let home = PathBuf::from("/home/you");
        assert_eq!(
            download_dir(Some(named.clone()), Some(home.clone())),
            Some(named)
        );
        assert_eq!(
            download_dir(None, Some(home)),
            Some(PathBuf::from("/home/you/Downloads"))
        );
        assert_eq!(download_dir(None, None), None);
    }

    #[test]
    fn a_second_download_of_a_name_overwrites_nothing() {
        let dir = std::env::temp_dir().join(format!("loca-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = std::ffi::OsStr::new("report.pdf");
        assert_eq!(unused_path(&dir, name), dir.join("report.pdf"));
        std::fs::write(dir.join("report.pdf"), b"1").unwrap();
        assert_eq!(unused_path(&dir, name), dir.join("report (1).pdf"));
        std::fs::write(dir.join("report (1).pdf"), b"2").unwrap();
        assert_eq!(unused_path(&dir, name), dir.join("report (2).pdf"));
        let bare = std::ffi::OsStr::new("README");
        std::fs::write(dir.join("README"), b"3").unwrap();
        assert_eq!(unused_path(&dir, bare), dir.join("README (1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_page_is_told_where_a_download_went() {
        let path = std::path::Path::new("/home/you/Downloads/report (1).pdf");
        assert_eq!(
            download_notice(Some(path), true),
            "saved to /home/you/Downloads/report (1).pdf"
        );
        assert_eq!(
            download_notice(None, true),
            "saved to your Downloads folder"
        );
        assert_eq!(
            download_notice(Some(path), false),
            "the download did not finish"
        );
    }

    #[test]
    fn the_window_navigates_only_within_the_app_origin() {
        let own = |path: &str| tauri::Url::parse(&format!("{APP_ORIGIN}{path}")).unwrap();
        assert!(may_navigate(&own("/")));
        assert!(may_navigate(&own("/index.html?x=1#y")));
        assert!(may_navigate(&tauri::Url::parse(APP_ORIGIN).unwrap()));
        // A blob the page itself made (an attachment saved or opened).
        let own_blob = format!("blob:{APP_ORIGIN}/0b5c1f2e-3d4a-4b6c-8d7e-9f0a1b2c3d4e");
        assert!(may_navigate(&tauri::Url::parse(&own_blob).unwrap()));
        for other in [
            "blob:https://loca.speakbetter.tech/0b5c1f2e-3d4a-4b6c-8d7e-9f0a1b2c3d4e",
            "blob:http://127.0.0.1:8787/0b5c1f2e-3d4a-4b6c-8d7e-9f0a1b2c3d4e",
            "blob:null/0b5c1f2e-3d4a-4b6c-8d7e-9f0a1b2c3d4e",
            "https://loca.speakbetter.tech/",
            "http://127.0.0.1:8787/",
            "http://tauri.localhost.example.com/",
            "tauri://localhost.evil/",
            "about:blank",
            "file:///etc/passwd",
        ] {
            assert!(
                !may_navigate(&tauri::Url::parse(other).unwrap()),
                "must refuse {other}"
            );
        }
        // The other OS's spelling of the app origin is someone else's here.
        let elsewhere = if cfg!(windows) {
            "tauri://localhost/"
        } else {
            "http://tauri.localhost/"
        };
        assert!(!may_navigate(&tauri::Url::parse(elsewhere).unwrap()));
    }

    // The Host's local ADMIN_TOKEN lives in the OS keychain under this key
    // (standalone::ADMIN_KEY). The raw master token must stay in Rust/keychain and
    // NEVER cross to the webview, the process argv, chat, or logs. These two
    // webview-boundary fences are mutation-verified: the admin-token key is NOT
    // among the values mirrored into the injected boot cache (KC_KEYS), and the
    // write command refuses it. KC_KEYS also gates kc_get, so the first assertion
    // is the read fence. (argv: the token is handed to the child via Command::env,
    // never an argument; chat/log: the desktop never posts or logs the token.)
    const ADMIN_TOKEN_KEY: &str = "loca-local-admin"; // must match standalone::ADMIN_KEY

    // The Host's Master session is renewed for the window the shell builds
    // and for no other label: a window added later must be let in on purpose.
    #[test]
    fn the_host_session_is_given_only_to_the_main_window() {
        assert!(is_host_window(MAIN_WINDOW));
        for other in ["", "Main", "main ", "main2", "settings", "popup"] {
            assert!(!is_host_window(other), "must refuse window {other:?}");
        }
    }

    #[test]
    fn admin_token_key_is_not_mirrored_into_the_webview() {
        assert!(
            !KC_KEYS.contains(&ADMIN_TOKEN_KEY),
            "the raw admin token key must never be injected into the webview boot cache"
        );
    }

    #[test]
    fn webview_cannot_write_the_admin_token() {
        // kc_set refuses any key outside KC_KEYS, so the webview cannot smuggle the
        // admin token (or any unknown key) into the keychain.
        assert!(kc_set(ADMIN_TOKEN_KEY.to_string(), "attacker".to_string()).is_err());
    }

    #[test]
    fn zoom_steps_are_bounded_and_reset_to_actual_size() {
        assert_eq!(next_zoom_index(0, -1), 0);
        assert_eq!(
            next_zoom_index(ZOOM_LEVELS.len() - 1, 1),
            ZOOM_LEVELS.len() - 1
        );
        assert_eq!(
            next_zoom_index(DEFAULT_ZOOM_INDEX, 1),
            DEFAULT_ZOOM_INDEX + 1
        );
        assert_eq!(
            next_zoom_index(DEFAULT_ZOOM_INDEX, -1),
            DEFAULT_ZOOM_INDEX - 1
        );
        assert_eq!(next_zoom_index(0, 0), DEFAULT_ZOOM_INDEX);
        assert_eq!(ZOOM_LEVELS[DEFAULT_ZOOM_INDEX], 1.0);
    }

    #[test]
    fn zoom_accelerators_are_accepted_by_the_runtime_parser() {
        use std::str::FromStr;

        for accelerator in [
            ZOOM_IN_ACCELERATOR,
            ZOOM_OUT_ACCELERATOR,
            ZOOM_RESET_ACCELERATOR,
        ] {
            assert!(
                muda::accelerator::Accelerator::from_str(accelerator).is_ok(),
                "unsupported desktop accelerator: {accelerator}"
            );
        }
    }
}
