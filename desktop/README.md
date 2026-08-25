# Loca Desktop (Tauri)

> **Scope — three distribution options (all one codebase):**
>
> 1. **Web** — Loca's primary, universal product. Installs nothing, runs
>    everywhere, connects to the shared hosted server. What the operators use.
> 2. **Desktop (client)** — a native window that ships **pre-pointed at the
>    shared hosted server** (no URL to type — see the baked `LOCA_DEFAULT_SERVER`
>    below), plus native OS notifications and OS-keychain credentials. Same
>    shared rooms as web, just as a real app. For non-developer users who want a
>    a downloadable, unsigned installer (see Faz 4).
> 3. **Desktop (full / standalone)** — the same app, but the `room-server` binary
>    is **bundled inside it** (Tauri sidecar) and booted locally on launch, so it
>    has **no dependency on an external server**. For solo / offline / LAN use.
>    Honest caveat: because Loca rooms are *shared*, others joining your rooms
>    over the internet still need your machine reachable (public IP / tunnel) —
>    that is the nature of self-hosting, not an extra flaw. Option 3 = "be your
>    own host".
>
> Options 2 and 3 are two **build flavors** of this crate (sidecar off / on),
> selected by a Cargo feature + `LOCA_DEFAULT_SERVER` at build time. Web stays
> the primary product; the desktop options sit beside it.

A native desktop shell around the **exact same** Loca web UI and the **exact
same** `room-server` backend. Zero UI/backend duplication: the desktop app is a
thin Tauri window that loads `web/` and talks to a configurable loca-server over
WS+REST, just like the browser client.

The public repository keeps web, server, and desktop sources together on its
single default branch. Desktop is an additive `desktop/` directory, not a
long-lived product branch.

## Architecture (thin wrapper)

```text
Tauri window  ──loads──▶  web/ (the same web UI, bundled)
       │
       └── web UI's serverBase() ──WS+REST──▶ configurable loca-server (room-server)
```

- `web/assets/state.js:serverBase()` already reads the `#server` input, so the
  UI is server-configurable out of the box. The only desktop shim needed is to
  **default that input to a remembered server** instead of `location.origin`
  (which, inside Tauri, is the app origin, not a loca-server). See Phase 1 TODO.

## Phases

- **Faz 0 — Setup** (this commit): `desktop/` crate + Tauri scaffold that bundles
  `web/` as the frontend. No workspace change (self-contained crate).
- **Faz 1 — Thin wrapper (MVP):** window loads the bundled web UI; persist the
  server URL, and — for the public build — **bake a default server** so the app
  opens already connected instead of showing an empty box. The compile-time
  `LOCA_DEFAULT_SERVER` (see `main.rs`) seeds the `#server` field when nothing is
  saved; `#server` becomes an advanced override rather than a first-run chore.
- **Faz 2 — Credential storage (SECURITY-CRITICAL, loca-dev review gate):**
  *code written, pending loca-dev build/review.* The two credential-bearing
  localStorage keys the web UI actually uses — `loca-seat` (davet/roomToken) and
  `loca-admin-session` (token) — are moved into the **OS keychain** (macOS
  Keychain / Windows Credential Manager / Linux Secret Service via `keyring`),
  never the webview's on-disk localStorage. A document-start Storage proxy
  intercepts only those two keys (everything else stays plain localStorage);
  Rust reads them from the keychain synchronously at window-build and injects
  the values so the web UI's **synchronous** boot read (`getItem("loca-seat")`)
  is answered with no async race. **Linux caveat:** the Secret Service needs a
  running keyring daemon (gnome-keyring/KWallet) — present on real desktops,
  possibly absent on a headless CI/build box, where a keychain read just returns
  empty and the user re-enters the seat once (the app still launches).
- **Faz 3 — Native notifications:** *code written, cargo-checked both flavors +
  a Playwright event spec; pending loca-dev runtime/privacy review.* Native OS
  notifications for the three notifiable events only — a **mention** (message
  addressed to me), a **directed attention**, and an **actionable reminder**;
  reactions and ordinary messages never notify. Boundary-respecting wiring: the
  shared `socket.js` calls a web-safe `notifyDesktop()` at those classification
  points (no-op in a plain browser), and the desktop injects `window.__LOCA_NOTIFY__`
  to fire a native notification via a custom Rust `notify` command (so no JS
  plugin capability is needed). **Privacy:** only the sender + event kind are
  forwarded and shown — never the message body — so a lock screen leaks no room
  text; `loca`/`id` ride along only for dedup + click-routing. **Foreground
  dedup:** the shim suppresses the OS notification while the window is focused
  (`document.hasFocus()`). **Event dedup:** `notifyDesktop()` keeps a seen-set
  keyed by kind+event-id (message id / attention id), so a repeated frame or a
  reconnect never double-notifies. The Rust `notify` command **whitelists** the
  kind (only `mention`/`attention`/`reminder`; else rejected). Tests:
  `tests/browser/notify-events.spec.js` pins the classification AND the dedup
  (duplicate frames add no notification; no body leaks).
  **Click-to-open — known constraint:** `tauri-plugin-notification`'s desktop
  `show()` is fire-and-forget (`notify_rust`, no click callback), so per-
  notification routing isn't available through the plugin. Options: `notify-rust`
  directly with an action handler (Linux-only today) or a future plugin
  capability — deferred and revisited with the per-OS work in Faz 4; the payload
  already carries `loca`/`id` so it can be wired later.
  *Runtime-pending (loca-dev):* the actual notification display/permission
  (Linux works without a prompt; macOS needs one).
- **Faz 5 — Standalone flavor (option 3), CLOSED-door:** ✅ **Linux GO** (loca-dev
  run-review, 2026-08-24): the Host opened, auto-connected to `general` as `you`,
  booted its sidecar on a dynamic `127.0.0.1` port (no `0.0.0.0`), used separate
  standalone app-data/SQLite, and cleaned up the child process + port on close —
  startup lock, provisioning, and resource bundle all verified at real runtime.
  Windows/macOS builds + signing still pending. The
  `bundled-server` Cargo feature boots a local `room-server` at launch and points
  the UI at it, all behind `#[cfg(feature = "bundled-server")]` so the client
  flavor stays byte-for-byte unchanged. It is **closed by default** (loca-dev's
  Faz 5a review correctly rejected the earlier open-mode plan):
  - **Dynamic port** — bind `127.0.0.1:0` for a free port; never a fixed,
    occupiable one (so the UI can't be pointed at a stranger's service).
  - **Readiness probe** — poll `/health` before the UI uses the server.
  - **Closed door** — random `ADMIN_TOKEN` in the OS keychain (reuses Faz 2, never
    reaches the webview) + `REQUIRE_INVITE=1` + `REQUIRE_SESSIONS=1`. This closes
    the loopback-CSWSH hole: a malicious browser page can open a WS to
    `127.0.0.1` but has no session/davet, so it's rejected.
  - **First-run provisioning** — admit the local user + issue a davet, store the
    seat exactly as the web UI reads it (`loca-seat = {name, roomToken, room}`),
    so it auto-connects. Idempotent by the seat's presence. Verified flow:
    `POST /members` → `POST /rooms/general/invites` → UI does `POST /sessions`.
  - **CORS** — `CORS_ALLOW_ORIGIN` set to the exact Tauri origins (never `*`),
    because the bundled UI runs from the Tauri origin, not `127.0.0.1`.
  - **Bound to `127.0.0.1` only, never `0.0.0.0`.** Persistent SQLite under the
    app data dir. Child stopped on app exit.
  - **loca-dev build:** first build the server (`cargo build -p server --release`
    from the repo root), then **stage the binary** into
    `desktop/src-tauri/binaries/` (its native name — `room-server` or, on Windows,
    `room-server.exe`; that dir is gitignored), then
    `cargo tauri build --features bundled-server --config tauri.standalone.conf.json`.
    The standalone config bundles `binaries/room-server*` (a glob, so the `.exe`
    suffix is handled on Windows) and it lands at `resource_dir()/binaries/<name>`
    — where `server_binary()` looks. Staging into a fixed `binaries/` dir keeps
    the resource path identical across OSes. **One thing to confirm on the
    target:** the exact `Origin` the webview sends (to finalize the
    `CORS_ALLOW_ORIGIN` allowlist — currently `tauri://localhost,http://tauri.localhost`).
  - **Faz 5b (later):** exposing the standalone beyond localhost (LAN/internet)
    stays a separate step; the honest default scope is local/offline (Scope caveat).
- **Faz 4 — Community distribution:** `.github/workflows/desktop-release.yml`
  builds both flavors on Linux, Windows, and macOS when a `desktop-v*` tag is
  pushed. It publishes unsigned `.msi`/`.exe`, `.dmg`, `.AppImage`, and `.deb`
  artifacts. No signing certificates, notarization accounts, updater keys, or
  repository secrets are required. Operating systems may display an unverified
  developer warning; users may alternatively build or sign the source locally.

## Build (loca-dev)

**Linux / Ubuntu prerequisites** (this is the environment we're demoing on —
`cargo tauri dev` fails without these WebKitGTK libs + the CLI):

```bash
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
cargo install tauri-cli --version '^2'   # provides `cargo tauri ...`
```

`libdbus-1-dev` is required by the Faz 2 keychain: keyring v3's
`sync-secret-service` backend links the system libdbus (via `dbus-secret-service`)
to reach the Linux Secret Service. Without it the build fails at link (an
isolated `cargo check` of the keyring API passes, since check does not link — so
the missing dev-lib only bites the full `cargo build`/`tauri build`).

Then:

```bash
# from repo root, on the `desktop` branch
cd desktop/src-tauri
cargo tauri dev      # run locally — opens the Loca window (needs a display)
cargo tauri build    # produce installers (needs icons/ generated first — see icons/GENERATE.md)
```

Notes for the first Linux run:

- This crate declares its own empty `[workspace]` (see `Cargo.toml`), so it
  builds standalone and does **not** touch the root `crates/*` workspace.
- `cargo tauri dev` serves the static `frontendDist` (`../../web`) directly — no
  separate dev server to start. The window should load the real Loca web UI and
  prompt for a server URL (the `#server` shim).
- Icons are only needed for `cargo tauri build` (bundling). Generate them once
  via `icons/GENERATE.md` before producing installers.

Version specifics (Tauri v1 vs v2 config keys) are for loca-dev to pin at build
time; this scaffold targets Tauri v2 and marks anything build-sensitive.

## Security notes (loca-care's gate on this branch)

1. **Credential storage** (Faz 2): OS keychain only, no plaintext secret on disk.
2. **Native notifications** (Faz 3): must not put raw message bodies/credentials
   into OS notification payloads beyond what the room already shows.
3. The shared auth model (principal / credential / davet) is **unchanged** — the
   desktop introduces no new auth surface, only a new local credential store.
