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
    std::fs::write(&temp, body).map_err(|e| format!("write host env {}: {e}", temp.display()))?;
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
            let seat: serde_json::Value = serde_json::from_str(&written).expect("valid seat JSON");
            assert_eq!(seat["room"], room, "from {previous:?}");
            assert_eq!(seat["name"], "you");
            assert_eq!(seat["roomToken"], "");
        }
    }
}
