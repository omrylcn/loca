// Loca desktop — thin Tauri wrapper.
//
// It bundles and loads the SAME web UI (../../web) and lets that UI talk to a
// configurable loca-server over WS+REST, exactly like the browser client. There
// is no forked UI and no forked backend here.
//
// Faz 1: window shell + the server-URL shim (SERVER_SHIM).
// Faz 2 (this file): move the two credential-bearing localStorage keys off the
//   webview's on-disk store into the OS keychain, WITHOUT forking the web UI.
//
//   The web UI (measured in web/assets/) persists exactly two sensitive keys:
//     - "loca-seat"          => { name, roomToken (=davet), room }   (api.js)
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

use tauri::{WebviewUrl, WebviewWindowBuilder};
#[cfg(feature = "bundled-server")]
use tauri::Manager; // for app.manage() in the standalone flavor

// Keychain service namespace + the exact credential keys mirrored from the web
// UI. Keep this list in sync with the localStorage keys above; any key NOT here
// is left in plain localStorage by the JS shim.
//
// The standalone flavor uses a SEPARATE namespace so its auto-provisioned local
// seat never collides with the client flavor's hosted seat on the same machine
// (a shared "loca-seat" would make standalone skip provisioning and hand the
// local server a foreign davet).
#[cfg(not(feature = "bundled-server"))]
const KC_SERVICE: &str = "tech.speakbetter.loca.desktop";
#[cfg(feature = "bundled-server")]
const KC_SERVICE: &str = "tech.speakbetter.loca.desktop.standalone";
const KC_KEYS: [&str; 2] = ["loca-seat", "loca-admin-session"];

// Baked at build time so the public desktop opens ALREADY pointed at a server
// instead of an empty box (the "esprisi yok" complaint). Two flavors set it
// differently at build:
//   - client flavor  : LOCA_DEFAULT_SERVER=https://loca.speakbetter.tech (the
//                       shared hosted server)
//   - full/standalone: LOCA_DEFAULT_SERVER=http://127.0.0.1:8787 (the local
//                       sidecar room-server this app boots)
// Unset -> empty -> the field just prompts (current dev behaviour). The user
// can always override via the #server field (persisted as loca-desktop-server).
const DEFAULT_SERVER: &str = match option_env!("LOCA_DEFAULT_SERVER") {
    Some(s) => s,
    None => "",
};

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
    kc_entry(&key)?.set_password(&value).map_err(|e| e.to_string())
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

// ── Faz 3: native notifications ─────────────────────────────────────────────
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

// ── Option 3 (standalone flavor): bundled room-server sidecar ────────────────
// Only compiled with `--features bundled-server`. Boots a LOCAL, CLOSED-door
// room-server on 127.0.0.1 at launch, provisions the user's own seat, and points
// the UI there — so the app has no dependency on an external server. The whole
// HTTP flow below was verified against a real closed-door room-server before it
// was written (admit -> davet -> session, HTTP 201).
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

    fn canonical_host_seat() -> String {
        format!(
            "{{\"name\":\"{LOCAL_NAME}\",\"roomToken\":\"\",\"room\":\"{LOCAL_ROOM}\"}}"
        )
    }

    fn provision_host<M, W>(mut mint_master: M, mut write_seat: W) -> Result<(), String>
    where
        M: FnMut() -> Result<(), String>,
        W: FnMut(&str) -> Result<(), String>,
    {
        mint_master()?;
        write_seat(&canonical_host_seat())
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
    // the Faz 2 store). It NEVER reaches the webview — only the davet does.
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
        let name = if cfg!(windows) { "room-server.exe" } else { "room-server" };
        let dir = app
            .path()
            .resource_dir()
            .map_err(|e| format!("resource_dir: {e}"))?;
        Ok(dir.join("binaries").join(name))
    }

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
        let admin = local_admin_token()?;
        let mut cmd = Command::new(&bin);
        cmd.env("BIND_ADDR", "127.0.0.1") // SECURITY: localhost only, never 0.0.0.0
            .env("PORT", port.to_string()) // dynamic: never a fixed, occupiable port
            .env("DB_PATH", &db) // persistent across restarts
            .env("ADMIN_TOKEN", &admin) // closed: admin actions need the token
            .env("REQUIRE_INVITE", "1") // closed: every loca needs a davet
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
            .env("CORS_ALLOW_ORIGIN", "tauri://localhost,http://tauri.localhost");
        // Windows: the room-server is a console binary; without this it pops a
        // separate terminal window next to the app. CREATE_NO_WINDOW keeps the
        // sidecar headless so the user sees only the app.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
        Ok((child, base))
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
    // may have left a davet-backed loca-seat in the OS keychain while its local
    // database was removed. Reusing that stale seat makes the Host choose the
    // davet before its freshly minted Master session and fall back to the door.
    // Always replace it with the canonical Host seat: iye navigation with no
    // davet. Identity and authority come exclusively from the derived Master
    // session below; the raw admin token never reaches the webview.
    pub fn provision_if_needed(base: &str) -> Result<(), String> {
        provision_host(
            // The owner of the local server is its MASTER — refresh the admin
            // session on every launch so master survives restart/expiry.
            || provision_master_session(base),
            // The seat carries only iye with an EMPTY davet. This write is
            // unconditional so a stale keychain seat cannot win after reinstall.
            |seat| {
                super::kc_entry("loca-seat")?
                    .set_password(seat)
                    .map_err(|e| e.to_string())
            },
        )
    }

    // Mint a time-limited admin session from the raw ADMIN_TOKEN and store it as
    // loca-admin-session (the UI reads it via state.js and opens master surfaces:
    // Lobby, This Loca/Call, master desk). The raw token never leaves
    // Rust; only the derived session (revocable, expiring) reaches the webview.
    pub fn provision_master_session(base: &str) -> Result<(), String> {
        let admin = local_admin_token()?;
        let resp = post_json(
            &format!("{base}/sessions"),
            &admin,
            &format!("{{\"name\":\"{LOCAL_NAME}\",\"kind\":\"user\"}}"),
        )
        .map_err(|e| format!("mint admin session: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&resp).map_err(|e| format!("parse session: {e}"))?;
        let token = v
            .get("session_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| format!("no session_token in {resp}"))?;
        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(LOCAL_NAME);
        // expires_at may be null (no expiry) -> a far-future stamp the UI accepts.
        let expires = v
            .get("expires_at")
            .and_then(|e| e.as_i64())
            .unwrap_or(4_102_444_800_000);
        let admin_session =
            format!("{{\"token\":\"{token}\",\"expiresAt\":{expires},\"name\":\"{name}\"}}");
        super::kc_entry("loca-admin-session")?
            .set_password(&admin_session)
            .map_err(|e| e.to_string())
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

    // Best-effort stop; called from the run-loop on exit so we don't orphan it.
    pub fn stop(app: &tauri::AppHandle) {
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
        fn host_provisioning_overwrites_a_stale_davet_seat() {
            for initial in [
                r#"{"name":"you","roomToken":"dv_stale","room":"old"}"#,
                r#"{"name":"you","roomToken":"","room":"iye"}"#,
            ] {
                let stored = std::cell::RefCell::new(initial.to_string());
                let mut minted = 0;
                let mut writes = 0;
                super::provision_host(
                    || {
                        minted += 1;
                        Ok(())
                    },
                    |seat| {
                        writes += 1;
                        *stored.borrow_mut() = seat.to_string();
                        Ok(())
                    },
                )
                .expect("host provisioning");

                assert_eq!(minted, 1, "Master session refreshes on every launch");
                assert_eq!(writes, 1, "Host seat is authoritative on every launch");
                let seat: serde_json::Value =
                    serde_json::from_str(&stored.borrow()).expect("valid seat JSON");
                assert_eq!(seat["name"], "you");
                assert_eq!(seat["room"], "iye");
                assert_eq!(seat["roomToken"], "", "no stale davet can be selected");
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
const SERVER_SHIM: &str = r#"
(function () {
  try {
    // Host mode (standalone flavor) is LOCKED to its bundled local server: a
    // saved remote override must never apply and manual changes are not
    // persisted, so the app can't be pointed away from its own sidecar.
    var locked = !!window.__LOCA_LOCK_SERVER__;
    // In the client flavor, a server the user saved before wins over the baked
    // default.
    var saved = null;
    try { saved = localStorage.getItem("loca-desktop-server"); } catch (e) {}
    if (!locked && saved && saved.trim()) { window.__LOCA_DEFAULT_SERVER__ = saved.trim(); }
    if (!locked) {
      // Persist future manual edits to the #server field once it exists.
      window.addEventListener("load", function () {
        try {
          var input = document.getElementById("server");
          if (!input) return;
          input.addEventListener("change", function () {
            try { localStorage.setItem("loca-desktop-server", (input.value || "").trim()); } catch (e) {}
          });
        } catch (e) {}
      });
    }
  } catch (e) { /* never let the shim break the app */ }
})();
"#;

// Storage proxy: for the credential keys, serve reads from the Rust-injected
// boot cache and route writes to the OS keychain; all other keys pass straight
// through to real localStorage. Runs at document-start, before the web UI's
// scripts, so the override is in place before the first getItem("loca-seat").
const KEYCHAIN_SHIM: &str = r#"
(function () {
  try {
    var KC_KEYS = ["loca-seat", "loca-admin-session"];
    var boot = window.__LOCA_KC_BOOT__ || {};
    var cache = {};
    KC_KEYS.forEach(function (k) { if (boot[k] != null) cache[k] = String(boot[k]); });
    try { delete window.__LOCA_KC_BOOT__; } catch (e) {}

    function invoke() {
      var t = window.__TAURI__;
      var fn = (t && t.core && t.core.invoke) || (t && t.invoke);
      return fn ? fn : null;
    }
    function isKc(k) { return KC_KEYS.indexOf(k) !== -1; }

    var proto = window.Storage && window.Storage.prototype;
    if (!proto) return; // no Web Storage -> nothing to guard
    var rawGet = proto.getItem, rawSet = proto.setItem, rawRemove = proto.removeItem;

    proto.getItem = function (k) {
      if (isKc(k)) return Object.prototype.hasOwnProperty.call(cache, k) ? cache[k] : null;
      return rawGet.call(this, k);
    };
    proto.setItem = function (k, v) {
      if (isKc(k)) {
        cache[k] = String(v);
        var fn = invoke();
        // If Tauri invoke isn't ready yet, keep it in the session cache only;
        // it is NEVER written to plain localStorage (no plaintext on disk).
        if (fn) { try { fn("kc_set", { key: k, value: String(v) }).catch(function () {}); } catch (e) {} }
        return;
      }
      return rawSet.call(this, k, v);
    };
    proto.removeItem = function (k) {
      if (isKc(k)) {
        delete cache[k];
        var fn = invoke();
        if (fn) { try { fn("kc_delete", { key: k }).catch(function () {}); } catch (e) {} }
        return;
      }
      return rawRemove.call(this, k);
    };
  } catch (e) { /* fail open: never let the shim break the app */ }
})();
"#;

// Bridges the web UI's notifyDesktop() calls to native OS notifications. Runs at
// document-start so window.__LOCA_NOTIFY__ exists before the first WS frame.
const NOTIFY_SHIM: &str = r#"
(function () {
  try {
    window.__LOCA_NOTIFY__ = function (ev) {
      try {
        // Foreground dedup: a focused window already shows the event in-app, so
        // don't also raise an OS notification (no double-notify).
        if (document.hasFocus && document.hasFocus()) return;
        var t = window.__TAURI__;
        var invoke = (t && t.core && t.core.invoke) || (t && t.invoke);
        if (!invoke) return;
        invoke("notify", { payload: ev }).catch(function () {});
      } catch (e) {}
    };
  } catch (e) { /* never let the shim break the app */ }
})();
"#;

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            kc_set,
            kc_get,
            kc_delete,
            notify
        ])
        .setup(|app| {
            // Standalone flavor: boot the local room-server BEFORE the window,
            // wait for it, and provision the user's seat so the UI connects to a
            // ready, closed-door server. Any failure here degrades gracefully — the
            // UI just shows a connect prompt rather than the app dying.
            #[cfg(feature = "bundled-server")]
            let standalone_url: Option<String> = match standalone::spawn(app) {
                Ok((mut child, base)) => {
                    // Check readiness (and child liveness) BEFORE moving the child
                    // into managed state.
                    if standalone::wait_ready(&base, &mut child) {
                        if let Err(e) = standalone::provision_if_needed(&base) {
                            eprintln!("standalone provisioning failed: {e}");
                        }
                    } else {
                        eprintln!("standalone server did not become ready (or exited)");
                    }
                    app.manage(standalone::ServerProc {
                        child: std::sync::Mutex::new(Some(child)),
                    });
                    Some(base)
                }
                Err(e) => {
                    eprintln!("bundled room-server failed to start: {e}");
                    None
                }
            };

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
            let boot_json = serde_json::to_string(&serde_json::Value::Object(boot))
                .unwrap_or_else(|_| "{}".to_string());
            // Standalone points the UI at the just-booted local server (dynamic
            // port); if it failed to start, fall back to the baked default. The
            // client flavor always uses the build-time baked default.
            #[cfg(feature = "bundled-server")]
            let effective_default = standalone_url.as_deref().unwrap_or(DEFAULT_SERVER);
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
            let init = format!(
                "window.__LOCA_LOCK_SERVER__ = {lock_server};\n\
                 window.__LOCA_DEFAULT_SERVER__ = {default_server};\n\
                 window.__LOCA_KC_BOOT__ = {boot_json};\n{KEYCHAIN_SHIM}\n{SERVER_SHIM}\n{NOTIFY_SHIM}"
            );

            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Loca")
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 520.0)
                .initialization_script(&init)
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

    // The Host's local ADMIN_TOKEN lives in the OS keychain under this key
    // (standalone::ADMIN_KEY). The raw master token must stay in Rust/keychain and
    // NEVER cross to the webview, the process argv, chat, or logs. These two
    // webview-boundary fences are mutation-verified: the admin-token key is NOT
    // among the values mirrored into the injected boot cache (KC_KEYS), and the
    // write command refuses it. KC_KEYS also gates kc_get, so the first assertion
    // is the read fence. (argv: the token is handed to the child via Command::env,
    // never an argument; chat/log: the desktop never posts or logs the token.)
    const ADMIN_TOKEN_KEY: &str = "loca-local-admin"; // must match standalone::ADMIN_KEY

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
}
