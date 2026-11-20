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
//     - "loca-admin-session" => { token, expiresAt }                 (state.js)
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
// BUILD NOTE for loca-dev: this targets the Tauri v2 builder API
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
// was given has ended (web/assets/state.js hostMasterSession). It is the start
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
mod standalone {
    use std::process::{Child, Command};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tauri::Manager;

    const ADMIN_KEY: &str = "loca-local-admin"; // keychain entry for the local master token
    const LOCAL_NAME: &str = "you"; // seat label for the Host's own Master session
                                    // The fixed private home loca present in every install: the Master (this
                                    // Host's owner) + this install's loca-care, and no one else. The app opens
                                    // straight into it.
    const LOCAL_ROOM: &str = "iye";

    fn canonical_host_seat(room: &str) -> String {
        serde_json::json!({ "name": LOCAL_NAME, "roomToken": "", "room": room }).to_string()
    }

    // The loca the Host's page goes back to: the last one its owner was in
    // (the page keeps it in the seat, web/assets/api.js rememberLastLoca), when
    // the remembered seat is the Host's own, with no invite; else iye. Only the
    // room is carried over, and the page checks it may still enter it
    // (landingRoom), falling back to iye.
    fn host_seat_room(previous: Option<&str>) -> String {
        previous
            .and_then(|seat| serde_json::from_str::<serde_json::Value>(seat).ok())
            .filter(|seat| seat.get("roomToken").and_then(|t| t.as_str()) == Some(""))
            .and_then(|seat| seat.get("room")?.as_str().map(str::to_string))
            .filter(|room| !room.is_empty())
            .unwrap_or_else(|| LOCAL_ROOM.to_string())
    }

    fn provision_host<M, W>(
        previous_seat: Option<&str>,
        mut mint_master: M,
        mut write_seat: W,
    ) -> Result<String, String>
    where
        M: FnMut() -> Result<String, String>,
        W: FnMut(&str) -> Result<(), String>,
    {
        let admin_session = mint_master()?;
        write_seat(&canonical_host_seat(&host_seat_room(previous_seat)))?;
        Ok(admin_session)
    }

    // Holds the child so the run-loop can stop it on exit. Managed as Tauri state.
    pub struct ServerProc {
        pub child: Mutex<Option<Child>>,
    }

    // Ask the OS for a free loopback port, then release it for the child to bind.
    // A tiny TOCTOU window exists (another process could grab it first); on
    // localhost + single-user that is acceptable, and the readiness probe catches
    // the rare miss (the child fails to bind, /health never answers).
    fn free_port() -> Result<u16, String> {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let p = l.local_addr().map_err(|e| e.to_string())?.port();
        Ok(p)
    }

    fn random_hex_32() -> String {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).expect("OS RNG unavailable");
        buf.iter().map(|b| format!("{b:02x}")).collect()
    }

    // Get-or-create the local server's master token in the OS keychain (reuses
    // the Phase 2 store). It NEVER reaches the webview — only the invite does.
    // FAIL-CLOSED: if a freshly minted token can't be persisted, return Err so
    // the caller aborts — silently continuing would mint a DIFFERENT token next
    // launch that no longer matches the persistent server DB's master.
    fn local_admin_token() -> Result<String, String> {
        if let Ok(Some(tok)) = super::kc_read(ADMIN_KEY) {
            if !tok.is_empty() {
                return Ok(tok);
            }
        }
        let tok = format!("adm_{}", random_hex_32());
        super::kc_entry(ADMIN_KEY)?
            .set_password(&tok)
            .map_err(|e| format!("cannot persist local admin token: {e}"))?;
        Ok(tok)
    }

    // Locate the room-server binary bundled alongside the app. The release
    // workflow stages it into `binaries/` and tauri.standalone.conf.json bundles
    // that dir (glob `binaries/room-server*`, so the platform's `.exe` suffix is
    // handled), which lands at `resource_dir()/binaries/<name>`.
    fn server_binary(app: &tauri::App) -> Result<std::path::PathBuf, String> {
        let name = if cfg!(windows) {
            "room-server.exe"
        } else {
            "room-server"
        };
        let dir = app
            .path()
            .resource_dir()
            .map_err(|e| format!("resource_dir: {e}"))?;
        Ok(dir.join("binaries").join(name))
    }

    // Starts the error spawn() returns when the OS keychain could not give or
    // keep the local admin token (locked, or no keyring service), so setup can
    // tell the owner what to fix instead of a general failure.
    pub const KEYCHAIN_FAILED: &str = "keychain unavailable: ";

    // Spawn the local server CLOSED by default. Returns (child, base_url) where
    // base_url is the dynamic 127.0.0.1:<port> the UI must use.
    pub fn spawn(app: &tauri::App) -> Result<(Child, String), String> {
        let bin = server_binary(app)?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("app_data_dir: {e}"))?;
        std::fs::create_dir_all(&data_dir).map_err(|e| format!("create data dir: {e}"))?;
        let db = data_dir.join("loca-standalone.sqlite3");
        let port = free_port()?;
        let base = format!("http://127.0.0.1:{port}");
        let _ = SERVER_URL.set(base.clone());
        let admin = local_admin_token().map_err(|e| format!("{KEYCHAIN_FAILED}{e}"))?;
        let mut cmd = Command::new(&bin);
        cmd.env("BIND_ADDR", "127.0.0.1") // SECURITY: localhost only, never 0.0.0.0
            .env("PORT", port.to_string()) // dynamic: never a fixed, occupiable port
            .env("DB_PATH", &db) // persistent across restarts
            .env("ADMIN_TOKEN", &admin) // closed: admin actions need the token
            .env("REQUIRE_INVITE", "1") // closed: every loca needs an invite
            .env("REQUIRE_SESSIONS", "1") // closed: posting needs a server-derived session
            .env("PUBLIC_SERVER_URL", &base)
            .env("LOCA_AGENT_ROOM", LOCAL_ROOM) // home loca = iye
            .env("RESERVED_LOCA", LOCAL_ROOM) // iye is reserved: not deletable/renamable
            // The one Master principal's display name: the Host owner is the
            // Master, not the lesser per-loca "operator". room-server reads this
            // in ensure_master_principal (sealed + verified end-to-end via
            // desktop/smoke/host_smoke.sh), so the admin session — and the whole
            // UI — shows a real Master identity. Honors an outer LOCA_MASTER_NAME
            // if the owner set one; else "Master".
            .env(
                "LOCA_MASTER_NAME",
                std::env::var("LOCA_MASTER_NAME").unwrap_or_else(|_| "Master".into()),
            )
            // The bundled UI runs from the Tauri origin, not 127.0.0.1, so REST is
            // cross-origin: allow the EXACT Tauri origins, never "*". (loca-dev to
            // confirm the exact Origin the webview sends per target platform.)
            .env(
                "CORS_ALLOW_ORIGIN",
                "tauri://localhost,http://tauri.localhost",
            );
        prepare(&mut cmd);
        // Kept so supervise() can start it again exactly like this.
        let _ = LAUNCH.set((
            bin.clone(),
            cmd.get_envs()
                .filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned())))
                .collect(),
        ));
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
        Ok((child, base))
    }

    fn prepare(cmd: &mut Command) {
        // Windows: the room-server is a console binary; without this it pops a
        // separate terminal window next to the app. CREATE_NO_WINDOW keeps the
        // sidecar headless so the user sees only the app.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // A Host ended by a signal (logout, `kill`, a crash) never reaches the
        // run-loop's stop(); without this its server keeps listening, owned
        // by nobody.
        #[cfg(target_os = "linux")]
        end_with_this_thread(cmd);
    }

    // The binary and environment the server was started with: same port,
    // database and token, so a restarted server answers at the address the
    // page already uses and the page reconnects on its own.
    #[allow(clippy::type_complexity)]
    static LAUNCH: std::sync::OnceLock<(
        std::path::PathBuf,
        Vec<(std::ffi::OsString, std::ffi::OsString)>,
    )> = std::sync::OnceLock::new();
    // Set by stop(): once the Host is closing, nothing is started again.
    static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    // The wait before the next start, and the one after it. `ran_for` is how
    // long the server that just exited had run, or None when the last start
    // itself failed. A server that ran for more than a minute was healthy:
    // the wait starts again at 1 s, once, for that exit. Otherwise (it keeps
    // exiting soon after a start, or a start keeps failing) the wait doubles,
    // up to 30 s.
    fn restart_wait(delay: &mut Duration, ran_for: Option<Duration>) -> Duration {
        if ran_for.is_some_and(|ran| ran > Duration::from_secs(60)) {
            *delay = Duration::from_secs(1);
        }
        let wait = *delay;
        *delay = (*delay * 2).min(Duration::from_secs(30));
        wait
    }

    // Start the server again whenever it exits while the Host runs, waiting
    // as restart_wait says. The thread lives as long as the Host, which on
    // Linux is also what keeps a restarted server tied to it
    // (end_with_this_thread).
    pub fn supervise(app: tauri::AppHandle) {
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::SeqCst;
            let mut delay = Duration::from_secs(1);
            let mut started = Instant::now();
            // The child in the state is still the one that exited when the
            // last start failed: its run time says nothing any more.
            let mut start_failed = false;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let Some(state) = app.try_state::<ServerProc>() else {
                    return;
                };
                let exited = match state.child.lock() {
                    Ok(mut guard) => match guard.as_mut() {
                        None => return, // stopped
                        Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                    },
                    Err(_) => return,
                };
                if !exited || STOPPING.load(SeqCst) {
                    continue;
                }
                let ran_for = (!start_failed).then(|| started.elapsed());
                let wait = restart_wait(&mut delay, ran_for);
                eprintln!("local server exited; restarting it in {}s", wait.as_secs());
                std::thread::sleep(wait);
                let Some((bin, envs)) = LAUNCH.get() else {
                    return;
                };
                let mut cmd = Command::new(bin);
                cmd.envs(envs.iter().map(|(k, v)| (k, v)));
                prepare(&mut cmd);
                match cmd.spawn() {
                    Ok(mut child) => {
                        started = Instant::now();
                        start_failed = false;
                        let Ok(mut guard) = state.child.lock() else {
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        };
                        if STOPPING.load(SeqCst) || guard.is_none() {
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                        *guard = Some(child);
                    }
                    Err(e) => {
                        start_failed = true;
                        eprintln!("restarting the local server failed: {e}");
                    }
                }
            }
        });
    }

    // Linux: the kernel sends the child SIGTERM when the thread that spawned
    // it ends. spawn() runs in setup on the main thread, which lives exactly
    // as long as the Host process, so the server goes when the Host goes,
    // however it goes.
    #[cfg(target_os = "linux")]
    fn end_with_this_thread(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: the closure runs in the forked child before exec and makes
        // only async-signal-safe calls (prctl, getppid); it does not allocate.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The Host may have gone between fork and prctl; then nobody
                // would ever send the signal.
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }

    // Poll /health until the child answers or we time out. Bails early if the
    // child has already exited (e.g. failed to bind the port) rather than waiting
    // out the full deadline for a process that is gone.
    pub fn wait_ready(base: &str, child: &mut Child) -> bool {
        let url = format!("{base}/health");
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(Some(_status)) = child.try_wait() {
                return false; // the server process died; stop waiting
            }
            if let Ok(resp) = ureq::get(&url).timeout(Duration::from_millis(500)).call() {
                if resp.status() == 200 {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }

    // Host provisioning is authoritative on every launch. A previous install
    // may have left an invite-backed loca-seat in the OS keychain while its local
    // database was removed. Reusing that stale seat makes the Host choose the
    // invite before its freshly minted Master session and fall back to the door.
    // Always replace it with the canonical Host seat: no invite, and the last
    // loca of the Host's own seat (else iye) to go back to. Identity and
    // authority come exclusively from the derived Master session below; the
    // raw admin token never reaches the webview.
    pub fn provision_if_needed(base: &str) -> Result<String, String> {
        let previous = super::kc_read("loca-seat").ok().flatten();
        provision_host(
            previous.as_deref(),
            // The owner of the local server is its MASTER — refresh the admin
            // session on every launch so master survives restart/expiry.
            || provision_master_session(base),
            // The seat carries only iye with an EMPTY invite. This write is
            // unconditional so a stale keychain seat cannot win after reinstall.
            |seat| {
                super::kc_entry("loca-seat")?
                    .set_password(seat)
                    .map_err(|e| e.to_string())
            },
        )
    }

    // The address of the server this Host started. Only that server is ever
    // asked for a new Master session (renew_master_session).
    static SERVER_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    // The page's Master session ended (its 12-hour lifetime ran out, or it was
    // ended on the server): mint the next one exactly as at start, from the
    // admin token in the keychain, at the server this Host started. The page
    // receives only the bounded session, as at start; the token stays here.
    pub fn renew_master_session() -> Result<String, String> {
        let base = SERVER_URL
            .get()
            .ok_or_else(|| "the Host's server was not started".to_string())?;
        provision_master_session(base)
    }

    // Mint a time-limited admin session from the raw ADMIN_TOKEN and store it as
    // loca-admin-session (the UI reads it via state.js and opens master surfaces:
    // Lobby, This Loca/Call, master desk). The raw token never leaves
    // Rust; only the derived session (revocable, expiring) reaches the webview.
    pub fn provision_master_session(base: &str) -> Result<String, String> {
        let admin = local_admin_token()?;
        let resp = post_json(
            &format!("{base}/sessions"),
            &admin,
            &format!("{{\"name\":\"{LOCAL_NAME}\",\"kind\":\"user\"}}"),
        )
        .map_err(|e| format!("mint admin session: {e}"))?;
        let admin_session = parse_master_session(&resp)?;
        // Persistence is recovery convenience, not an authority boundary. The
        // freshly minted bounded session is returned to this launch even when
        // keychain persistence is unavailable; the raw ADMIN_TOKEN stays here.
        match super::kc_entry("loca-admin-session") {
            Ok(entry) => {
                if let Err(e) = entry.set_password(&admin_session) {
                    eprintln!("cannot persist bounded admin session: {e}");
                }
            }
            Err(e) => eprintln!("cannot open bounded admin session keychain entry: {e}"),
        }
        Ok(admin_session)
    }

    // Pure: validates the minted session and builds the value the webview
    // reads. It touches no keychain, so its unit test cannot reach the
    // machine's real one.
    fn parse_master_session(resp: &str) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(resp).map_err(|e| format!("parse session: {e}"))?;
        if v.get("admin").and_then(|a| a.as_bool()) != Some(true) {
            return Err("minted session is not an admin session".to_string());
        }
        let token = v
            .get("session_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| format!("no session_token in {resp}"))?;
        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(LOCAL_NAME);
        // A Master stand-in must be finite. Never turn a missing/null expiry into
        // an invented far-future timestamp in the privileged desktop client.
        let expires = v
            .get("expires_at")
            .and_then(|e| e.as_i64())
            .ok_or_else(|| "admin session has no finite expires_at".to_string())?;
        Ok(format!(
            "{{\"token\":\"{token}\",\"expiresAt\":{expires},\"name\":\"{name}\"}}"
        ))
    }

    fn post_json(url: &str, admin: &str, body: &str) -> Result<String, String> {
        ureq::post(url)
            .set("x-admin-token", admin)
            .set("content-type", "application/json")
            .timeout(Duration::from_secs(5))
            .send_string(body)
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())
    }

    // The only server the Host's page may talk to: the one this Host started.
    // When it did not start there is none, never a baked or default address.
    pub fn page_server(started: Option<&str>) -> &str {
        started.unwrap_or("")
    }

    pub fn write_host_env(
        data_dir: &std::path::Path,
        server: &str,
        library: Option<&str>,
        provisioning_state: &str,
        provisioning_stage: &str,
    ) -> Result<(), String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| format!("create host data dir {}: {e}", data_dir.display()))?;
        let target = data_dir.join("host.env");
        let temp = data_dir.join(format!(".host.env-{}.tmp", std::process::id()));
        let body = format!(
            "ROOM_SERVER_URL={server}\nLOCA_SKILL_LIBRARY={}\n\
             HOST_PROVISIONING_STATE={provisioning_state}\n\
             HOST_PROVISIONING_STAGE={provisioning_stage}\n",
            library.unwrap_or("")
        );
        std::fs::write(&temp, body)
            .map_err(|e| format!("write host env {}: {e}", temp.display()))?;
        std::fs::rename(&temp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!(
                "promote host env {} -> {}: {e}",
                temp.display(),
                target.display()
            )
        })
    }

    // Best-effort stop; called from the run-loop on exit so we don't orphan it.
    pub fn stop(app: &tauri::AppHandle) {
        STOPPING.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(state) = app.try_state::<ServerProc>() {
            if let Ok(mut guard) = state.child.lock() {
                if let Some(mut child) = guard.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn master_session_requires_admin_and_finite_expiry() {
            let good = super::parse_master_session(
                r#"{"session_token":"sess","name":"you","admin":true,"expires_at":2000000000000}"#,
            )
            .expect("valid bounded admin session");
            assert!(good.contains(r#""token":"sess""#));
            assert!(good.contains(r#""expiresAt":2000000000000"#));

            for invalid in [
                r#"{"session_token":"sess","admin":false,"expires_at":2000000000000}"#,
                r#"{"session_token":"sess","expires_at":2000000000000}"#,
                r#"{"session_token":"sess","admin":true,"expires_at":null}"#,
                r#"{"session_token":"sess","admin":true}"#,
            ] {
                assert!(
                    super::parse_master_session(invalid).is_err(),
                    "must reject unbounded or non-admin session: {invalid}"
                );
            }
        }

        // The mechanism stop() cannot provide: the server child is signalled
        // when the thread that started it ends, even though nothing ran a
        // shutdown path.
        #[cfg(target_os = "linux")]
        #[test]
        fn server_child_ends_when_the_thread_that_started_it_ends() {
            use std::os::unix::process::ExitStatusExt;
            let mut child = std::thread::spawn(|| {
                let mut cmd = std::process::Command::new("sleep");
                cmd.arg("30");
                super::end_with_this_thread(&mut cmd);
                cmd.spawn().expect("spawn sleep")
            })
            .join()
            .expect("spawning thread");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().expect("try_wait") {
                    break Some(status);
                }
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            assert_eq!(
                status.and_then(|s| s.signal()),
                Some(libc::SIGTERM),
                "the child must be ended with SIGTERM once its starting thread is gone"
            );
        }

        // After a healthy run (longer than a minute) the first restart waits
        // 1 s; starts that keep failing then wait 2, 4, 8, ... up to 30 s,
        // not 1 s each: the reset after the healthy run applies once.
        #[test]
        fn failed_restarts_after_a_healthy_run_back_off() {
            use std::time::Duration;
            let mut delay = Duration::from_secs(8);
            let mut waits = vec![super::restart_wait(
                &mut delay,
                Some(Duration::from_secs(90)),
            )];
            for _ in 0..7 {
                waits.push(super::restart_wait(&mut delay, None));
            }
            let secs: Vec<u64> = waits.iter().map(|w| w.as_secs()).collect();
            assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
            // A server that keeps exiting soon after a start backs off too.
            let mut delay = Duration::from_secs(1);
            let secs: Vec<u64> = (0..4)
                .map(|_| super::restart_wait(&mut delay, Some(Duration::from_secs(2))).as_secs())
                .collect();
            assert_eq!(secs, [1, 2, 4, 8]);
            // A later healthy run starts the waits again.
            assert_eq!(
                super::restart_wait(&mut delay, Some(Duration::from_secs(61))).as_secs(),
                1
            );
        }

        #[test]
        fn host_page_talks_only_to_the_server_the_host_started() {
            assert_eq!(
                super::page_server(Some("http://127.0.0.1:41234")),
                "http://127.0.0.1:41234"
            );
            assert_eq!(super::page_server(None), "", "no server: no address at all");
        }

        #[test]
        fn host_env_is_atomically_published_with_runtime_values() {
            let root = std::env::temp_dir().join(format!(
                "loca-host-env-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            super::write_host_env(
                &root,
                "http://127.0.0.1:62103",
                Some("/skills/0.9.20"),
                "ready",
                "none",
            )
            .expect("write host env");
            assert_eq!(
                std::fs::read_to_string(root.join("host.env")).unwrap(),
                "ROOM_SERVER_URL=http://127.0.0.1:62103\n\
                 LOCA_SKILL_LIBRARY=/skills/0.9.20\n\
                 HOST_PROVISIONING_STATE=ready\n\
                 HOST_PROVISIONING_STAGE=none\n"
            );
            assert!(
                std::fs::read_dir(&root).unwrap().all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")),
                "atomic publish must not leak temp files"
            );
            super::write_host_env(
                &root,
                "http://127.0.0.1:62104",
                None,
                "error",
                "skill_library",
            )
            .expect("write diagnostic host env without a skill library");
            assert_eq!(
                std::fs::read_to_string(root.join("host.env")).unwrap(),
                "ROOM_SERVER_URL=http://127.0.0.1:62104\n\
                 LOCA_SKILL_LIBRARY=\n\
                 HOST_PROVISIONING_STATE=error\n\
                 HOST_PROVISIONING_STAGE=skill_library\n"
            );
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn host_provisioning_overwrites_a_stale_invite_seat() {
            for initial in [
                r#"{"name":"you","roomToken":"dv_stale","room":"old"}"#,
                r#"{"name":"you","roomToken":"","room":"iye"}"#,
            ] {
                let stored = std::cell::RefCell::new(initial.to_string());
                let mut minted = 0;
                let mut writes = 0;
                let previous = stored.borrow().clone();
                let session = super::provision_host(
                    Some(&previous),
                    || {
                        minted += 1;
                        Ok("bounded-session".to_string())
                    },
                    |seat| {
                        writes += 1;
                        *stored.borrow_mut() = seat.to_string();
                        Ok(())
                    },
                )
                .expect("host provisioning");

                assert_eq!(session, "bounded-session");
                assert_eq!(minted, 1, "Master session refreshes on every launch");
                assert_eq!(writes, 1, "Host seat is authoritative on every launch");
                let seat: serde_json::Value =
                    serde_json::from_str(&stored.borrow()).expect("valid seat JSON");
                assert_eq!(seat["name"], "you");
                assert_eq!(seat["room"], "iye");
                assert_eq!(seat["roomToken"], "", "no stale invite can be selected");
            }
        }

        #[test]
        fn host_provisioning_keeps_the_last_loca_of_its_own_seat() {
            for (previous, room) in [
                (
                    Some(r#"{"name":"Master","roomToken":"","room":"demo"}"#),
                    "demo",
                ),
                (
                    Some(r#"{"name":"you","roomToken":"dv_stale","room":"old"}"#),
                    "iye",
                ),
                (Some(r#"{"name":"Master","roomToken":"","room":""}"#), "iye"),
                (
                    Some(r#"{"name":"Master","roomToken":"","room":null}"#),
                    "iye",
                ),
                (Some("not json"), "iye"),
                (None, "iye"),
            ] {
                let mut written = String::new();
                super::provision_host(
                    previous,
                    || Ok("bounded-session".to_string()),
                    |seat| {
                        written = seat.to_string();
                        Ok(())
                    },
                )
                .expect("host provisioning");
                let seat: serde_json::Value =
                    serde_json::from_str(&written).expect("valid seat JSON");
                assert_eq!(seat["room"], room, "from {previous:?}");
                assert_eq!(seat["name"], "you");
                assert_eq!(seat["roomToken"], "");
            }
        }
    }
}

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
