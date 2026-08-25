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

// Finding 1 fix: the in-app "Add agent" flow for the Host. The webview passes a
// name + kind; the Host mints a membership + davet against its local server and
// returns ONLY the davet (+ the ready-to-paste server URL). The ADMIN_TOKEN
// never crosses to the webview — it stays in the keychain, read Rust-side.
// Available only in the standalone Host build; the client build returns an error.
#[tauri::command]
fn host_add_agent(
    _app: tauri::AppHandle,
    _name: String,
    _kind: String,
    _target: Option<String>,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "bundled-server")]
    {
        use tauri::Manager;
        let state = _app
            .try_state::<standalone::ServerProc>()
            .ok_or_else(|| "local server is not running".to_string())?;
        let base = state.base.clone();
        standalone::add_agent(&base, &_name, &_kind, _target.as_deref())
    }
    #[cfg(not(feature = "bundled-server"))]
    {
        Err("adding agents is only available in the standalone Host build".to_string())
    }
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

    // Holds the child (so the run-loop can stop it on exit) + the local base URL
    // (so the host_add_agent command can mint credentials against it). Managed
    // as Tauri state.
    pub struct ServerProc {
        pub child: Mutex<Option<Child>>,
        pub base: String,
    }

    // Onboard an agent the Host Master names, following the three credential
    // layers (operator's model, iye 2026-08-25):
    //   * a **Lobby davet** (the mb_ membership) admits the agent to the Building
    //     and leaves it waiting in the Lobby until it is called; and
    //   * a **Loca davet** (a dv_) seats it in one EXISTING loca.
    // If a target loca is named but does not exist, the agent keeps ONLY the
    // Lobby davet and waits in the Lobby — the Host never auto-creates a loca to
    // satisfy an invite, and never changes the target. ONLY the davet (mb_ or
    // dv_) is returned to the webview; the ADMIN_TOKEN never leaves Rust. The
    // admit endpoint is Master-only server-side, so a plain agent (loca-care)
    // cannot reach this — it can only ASK the Master to run it.
    pub fn add_agent(
        base: &str,
        name: &str,
        kind: &str,
        target_loca: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err("agent name must be 1-64 ASCII letters, digits, dot, dash, or underscore".into());
        }
        let kind = if kind == "user" { "user" } else { "agent" };
        let admin = local_admin_token()?;
        let body = serde_json::json!({"name": name, "kind": kind}).to_string();
        // Layer 2 — Lobby davet: admit to the Building. The returned mb_ token IS
        // the Lobby davet (the agent waits online in the Lobby until called).
        let member_resp = post_json(&format!("{base}/members"), &admin, &body)
            .map_err(|e| format!("admit: {e}"))?;
        let lobby_davet = json_field(&member_resp, "token")
            .ok_or_else(|| format!("admit returned no token: {member_resp}"))?;
        // Layer 3 — Loca davet: only when a real, existing loca is named.
        // NOTE (verified against room-server): the server mints a davet even for
        // a loca that does NOT exist (POST /rooms/<ghost>/invites -> 200 dv_), so
        // we MUST pre-check existence — otherwise a typo'd name seats the agent
        // in a phantom loca instead of the Lobby.
        if let Some(room) = target_loca.map(str::trim).filter(|r| !r.is_empty()) {
            if room.len() > 64
                || !room
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err("loca name must be 1-64 ASCII letters, digits, dot, dash, or underscore".into());
            }
            // iye is the reserved home loca (Master + loca-care only). The server
            // refuses to seat anyone else there; keep the agent in the Lobby with
            // an honest message rather than surfacing the reserved-loca error.
            if room.eq_ignore_ascii_case(LOCAL_ROOM) && name != "loca-care" {
                return Ok(serde_json::json!({
                    "name": name, "layer": "lobby", "room": room, "reserved": true,
                    "davet": lobby_davet, "server": base,
                }));
            }
            if loca_exists(base, &admin, room)? {
                let davet_resp = post_json(&format!("{base}/rooms/{room}/invites"), &admin, &body)
                    .map_err(|e| format!("invite: {e}"))?;
                let davet = json_field(&davet_resp, "token")
                    .ok_or_else(|| format!("invite returned no token: {davet_resp}"))?;
                return Ok(serde_json::json!({
                    "name": name, "layer": "loca", "room": room,
                    "davet": davet, "server": base,
                }));
            }
            // Named but absent -> Lobby only. Never auto-create the loca.
            return Ok(serde_json::json!({
                "name": name, "layer": "lobby", "room": room, "absent": true,
                "davet": lobby_davet, "server": base,
            }));
        }
        // No target -> Lobby davet.
        Ok(serde_json::json!({
            "name": name, "layer": "lobby",
            "davet": lobby_davet, "server": base,
        }))
    }

    // Does a loca with this exact name exist on the local server? Uses the admin
    // token (which never leaves Rust) to read the room list. A read failure is an
    // error (fail-closed: we do not fabricate a loca davet against an unknown
    // room); an unknown room is a clean `false`, so the agent stays in the Lobby.
    fn loca_exists(base: &str, admin: &str, room: &str) -> Result<bool, String> {
        let resp = ureq::get(&format!("{base}/rooms"))
            .set("x-admin-token", admin)
            .timeout(Duration::from_secs(5))
            .call()
            .map_err(|e| format!("list rooms: {e}"))?
            .into_string()
            .map_err(|e| e.to_string())?;
        let v: serde_json::Value =
            serde_json::from_str(&resp).map_err(|e| format!("parse rooms: {e}"))?;
        Ok(v.as_array()
            .map(|arr| {
                arr.iter()
                    .any(|r| r.get("room").and_then(|x| x.as_str()) == Some(room))
            })
            .unwrap_or(false))
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
    // Lobby, This Loca/Call, master desk, Add agent). The raw token never leaves
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

    // Extract one top-level string field without a dedicated deserialize target.
    fn json_field(json: &str, field: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        v.get(field)?.as_str().map(|s| s.to_string())
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

// Host-only "Add agent" panel. Injected by the desktop shell — it does NOT fork
// the shared web UI. It encodes the operator's three-layer model directly: as
// the Master, you either admit an agent to the Building (a Lobby davet — it
// waits until called) or invite it straight into an EXISTING loca (a Loca
// davet). Naming a loca that doesn't exist yet keeps the agent in the Lobby.
// Only the resulting davet is shown; the ADMIN_TOKEN never reaches here.
const HOST_SHIM: &str = r##"
(function () {
  try {
    if (!window.__LOCA_HOST__) return;
    function invoke() { var t = window.__TAURI__; return (t && t.core && t.core.invoke) || (t && t.invoke); }
    window.addEventListener("load", function () {
      try {
        var bar = document.createElement("div");
        bar.style.cssText = "position:fixed;right:14px;bottom:14px;z-index:99999;font:13px system-ui,sans-serif";
        bar.innerHTML =
          '<button id="_lc_add" style="padding:8px 12px;border-radius:8px;border:0;background:#2ee6c8;color:#04201b;cursor:pointer;font-weight:600">+ Add agent</button>' +
          '<div id="_lc_box" style="display:none;margin-top:8px;width:340px;background:#0b0e12;color:#e8eef2;border:1px solid #2ee6c8;border-radius:10px;padding:12px">' +
          '<div style="margin-bottom:6px;font-weight:600">Onboard an agent</div>' +
          '<div style="margin-bottom:8px;color:#8b97a2;font-size:12px">You are the Master here. Admit an agent to the Building (it waits in the Lobby), or invite it straight into an existing loca.</div>' +
          '<input id="_lc_name" placeholder="agent name, e.g. loca-care" style="width:100%;box-sizing:border-box;padding:6px;border-radius:6px;border:1px solid #2a3138;background:#10141a;color:#e8eef2"/>' +
          '<input id="_lc_room" placeholder="invite to loca (blank = Lobby)" style="margin-top:6px;width:100%;box-sizing:border-box;padding:6px;border-radius:6px;border:1px solid #2a3138;background:#10141a;color:#e8eef2"/>' +
          '<button id="_lc_go" style="margin-top:8px;padding:6px 10px;border-radius:6px;border:0;background:#2ee6c8;color:#04201b;cursor:pointer;font-weight:600">Create davet</button>' +
          '<pre id="_lc_out" style="display:none;white-space:pre-wrap;word-break:break-all;margin-top:8px;background:#10141a;padding:8px;border-radius:6px;font-size:12px"></pre></div>';
        document.body.appendChild(bar);
        var box = bar.querySelector("#_lc_box"), out = bar.querySelector("#_lc_out");
        bar.querySelector("#_lc_add").addEventListener("click", function () {
          box.style.display = box.style.display === "none" ? "block" : "none";
        });
        bar.querySelector("#_lc_go").addEventListener("click", function () {
          var name = (bar.querySelector("#_lc_name").value || "").trim();
          var room = (bar.querySelector("#_lc_room").value || "").trim();
          if (!name) return;
          var fn = invoke(); if (!fn) return;
          fn("host_add_agent", { name: name, kind: "agent", target: room || null }).then(function (r) {
            out.style.display = "block";
            var setup = "connect.sh setup " + r.server + " " + r.name;
            var head, tail = "";
            if (r.layer === "loca") {
              head = 'Agent "' + r.name + '" invited to loca "' + r.room + '" — it joins that loca directly.';
            } else if (r.reserved) {
              head = '"' + r.room + '" is the reserved home loca (Master + loca-care only), so "' + r.name + '" was admitted to the Building and waits in the Lobby.';
              tail = "\n\nCall it into one of your own locas from This Loca > + call.";
            } else if (r.absent) {
              head = 'Loca "' + r.room + '" does not exist yet, so "' + r.name + '" was admitted to the Building and waits in the Lobby.';
              tail = "\n\nCall it into a loca later from This Loca > + call.";
            } else {
              head = 'Agent "' + r.name + '" admitted to the Building — it waits in the Lobby until you call it into a loca.';
              tail = "\n\nCall it in later from This Loca > + call.";
            }
            out.textContent = head + "\n\nOn its machine, install both skills, then run:\n\n" + setup + "\n\nPaste this davet at the hidden prompt:\n" + r.davet + tail;
          }).catch(function (e) { out.style.display = "block"; out.textContent = "Failed: " + e; });
        });
      } catch (e) {}
    });
  } catch (e) { /* never let the shim break the app */ }
})();
"##;

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            kc_set,
            kc_get,
            kc_delete,
            notify,
            host_add_agent
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
                        base: base.clone(),
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
            // Standalone flavor exposes the in-app "Add agent" affordance.
            #[cfg(feature = "bundled-server")]
            let is_host = "true";
            #[cfg(not(feature = "bundled-server"))]
            let is_host = "false";
            let init = format!(
                "window.__LOCA_HOST__ = {is_host};\n\
                 window.__LOCA_LOCK_SERVER__ = {lock_server};\n\
                 window.__LOCA_DEFAULT_SERVER__ = {default_server};\n\
                 window.__LOCA_KC_BOOT__ = {boot_json};\n{KEYCHAIN_SHIM}\n{SERVER_SHIM}\n{NOTIFY_SHIM}\n{HOST_SHIM}"
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
