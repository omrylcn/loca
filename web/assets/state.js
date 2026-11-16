"use strict";
// Shared browser state, identity, room navigation, and unread cursors.
const $ = (id) => document.getElementById(id);
const state = { server: "", name: "operator", room: null, rooms: [], ws: null, members: [], lobby: [], tab: "chat", sidebarView: "building", locaOperator: null, locaContext: null, principalId: null, roomPreferences: { pinned: [], hidden: [], order: [], collapsed: [] }, notes: {}, editing: null, pairing: "", roomToken: "", session: null, authStatus: "unknown", adminSession: false, sessionExpires: null, profile: null, credentials: [], epoch: null, homeRoom: "iye", locaAgents: [], mode: { mode: "free" }, settings: { rate_limit: 10, rate_window_secs: 30 }, mod: { muted: [], banned: [] }, tasks: {}, goals: {}, attentions: {}, waits: {}, journal: [], lastId: 0, seen: new Set(), msgs: [], replyTo: null, reminderReceipts: new Set(), unread: {}, readCursors: {}, roomLatest: {}, unreadChecked: {}, readStorageKey: "", pendingAttachments: [], pinned: null, pinnedExpanded: false };
Object.assign(state, { memory: null, memoryEntries: [], memoryStatus: "loading", memoryError: "" });
// Building data belongs to whoever was signed in when it was asked for.
// Sign-out moves this generation on (app.js); every Building fetch notes it
// when it starts and writes nothing once it has moved, so an answer still in
// flight at sign-out cannot bring the last person's data back after the clear.
state.buildingGen = 0;
function buildingStale(gen) { return gen !== state.buildingGen; }

// The same rule, enforced where every request leaves the page. A request
// belongs to the generation it was sent in. When the generation moves on,
// whatever is still out is cancelled: the browser drops the request, and code
// waiting for the answer or for the rest of its body gets the AbortError any
// cancelled fetch gives, at that moment, before the next person has signed
// in. An answer for the last person therefore cannot arrive in the next
// person's page, whether or not the code that asked for it checks the
// generation itself.
//
// What this cannot stop is code that was not waiting on the network when the
// generation moved (a timer, a failure it swallowed) and then starts
// something new: that still has to check buildingStale() itself before it
// acts, and the checks at those places stay.
//
// One request is exempt, and only for this reason: the session exchange
// (POST /sessions in takeSessionFor). Its answer creates state on the server
// that only the token in that answer can end, and the server has usually
// made it before a cancel could arrive. Cancelled, the page would never learn
// the token and the session would stay valid, held by nobody. So it goes out
// with pageFetch, its answer is read whenever it can be, and an answer that
// is no longer current is applied to nothing and ended (endUnheldSession;
// what that still cannot end is listed there). Ending a
// session (DELETE /sessions) goes out the same way, so a sign-in cannot
// cancel it. No other request is exempt: an answer that only reads, or a
// change whose result the page can drop, is cancelled like any other.
let generationRequests = new AbortController();
const pageFetch = window.fetch.bind(window);
function eitherSignal(own, ours) {
  if (!own) return ours;
  if (typeof AbortSignal.any === "function") return AbortSignal.any([own, ours]);
  // Older webviews: the same thing by hand.
  const both = new AbortController();
  const stop = (signal) => () => both.abort(signal.reason);
  for (const signal of [own, ours]) {
    if (signal.aborted) { both.abort(signal.reason); break; }
    signal.addEventListener("abort", stop(signal), { once: true });
  }
  return both.signal;
}
window.fetch = (input, init) => pageFetch(input, {
  ...(init || {}),
  signal: eitherSignal(init && init.signal, generationRequests.signal),
});
// The one way a generation ends.
function moveBuildingOn() {
  state.buildingGen++;
  generationRequests.abort();
  generationRequests = new AbortController();
}
// A cancelled request that nobody was catching is not an error to report.
window.addEventListener("unhandledrejection", (event) => {
  if (event.reason && event.reason.name === "AbortError") event.preventDefault();
});

// A failure the owner can fix names its cause and the fix; the others keep
// the general message. The stages come from the desktop Host (main.rs).
const HOST_PROVISIONING_REMEDIES = {
  keychain: "The system keyring is locked or not running, so Loca Host did not start its server. Unlock the keyring (GNOME: Passwords and Keys), then restart Loca Host.",
};
function renderHostProvisioningStatus() {
  const box = $("hostProvisionError");
  if (!box) return;
  const status = window.__LOCA_HOST_PROVISIONING__;
  const failed = status && status.state === "error";
  box.textContent = failed
    ? HOST_PROVISIONING_REMEDIES[status.stage]
      || "Master session could not be prepared. Building administration is unavailable. Restart Loca Host to retry."
    : "";
  box.classList.toggle("hidden", !failed);
}

window.addEventListener("DOMContentLoaded", renderHostProvisioningStatus);

function setMobileSidebar(open) {
  document.body.classList.toggle("sidebar-open", !!open);
  $("sideToggle").setAttribute("aria-expanded", String(!!open));
  $("sideToggle").setAttribute(
    "aria-label", open ? "Close navigation" : "Open navigation",
  );
}

function adminHeaders(base) {
  const h = base || {};
  // Once a session exists it is the only credential sent. In particular, the
  // root/bootstrap credential must not keep riding along with every browser request.
  if (state.session) {
    h["x-session-token"] = state.session;
  } else if (state.roomToken) {
    h["x-room-token"] = state.roomToken;
  }
  return h;
}

// Exchange the room key for a session (server-derived identity). Admin
// sessions persist across deploys until their selected expiry; invite sessions
// can be renewed from their local invite after a restart.
//
// An attempt belongs to the person, key and server it was started for. Its
// answer may arrive after that person signed out and someone else signed in;
// applied then, it would put the last person's session and name over the new
// one's. So every answer is checked against the generation and server it was
// asked under, and one that is no longer theirs writes nothing: not the
// identity, not the stored session, not a verdict.
//
// Resolves true when the outcome is this page's current identity state (read
// it from state.authStatus as before), false when the attempt was overtaken
// and the caller must stop: whatever it was doing was for someone who is gone.
let sessionTaking = null;   // { key, promise } while an attempt is out
function takeSession() {
  // Room navigation is not a new login. Keep the already-bound identity;
  // clearing it here made every loca click throw the master back to the door.
  if (!state.roomToken && !state.pairing && state.session) {
    state.authStatus = "authenticated";
    return Promise.resolve(true);
  }
  const gen = state.buildingGen;
  const server = serverBase();
  // The same person asking again while their attempt is out joins it. Two
  // exchanges of one one-use pairing code would end with the second one's
  // refusal written over the first one's session.
  const key = JSON.stringify([gen, server, state.name, state.roomToken, state.pairing]);
  if (sessionTaking && sessionTaking.key === key) return sessionTaking.promise;
  const current = () => !buildingStale(gen) && server === serverBase();
  const taking = { key, promise: null };
  taking.promise = takeSessionFor(server, current).finally(() => {
    if (sessionTaking === taking) sessionTaking = null;
  });
  sessionTaking = taking;
  return taking.promise;
}
async function takeSessionFor(server, current, hostRenewed = false) {
  state.authStatus = "unknown";
  state.session = null;
  state.adminSession = false;
  state.sessionExpires = null;
  if (!state.roomToken && !state.pairing) {
    // Whether this page still held a stored Master session when the attempt
    // began. Loca Host replaces one that has ended (hostMasterSession); it
    // gives none to a page that holds none: a person who signed out stays
    // signed out, through a reload too.
    let heldStored = false;
    // Keep the expiring stand-in for exactly the lifetime selected at pairing.
    // This is NOT the root key: it is revocable server-side and carries its own
    // absolute expiry. localStorage lets that chosen lifetime survive closing
    // the tab/browser; logout and 401 remove it immediately.
    try {
      const cached = JSON.parse(
        localStorage.getItem("loca-admin-session")
        || sessionStorage.getItem("loca-admin-session")
        || "null"
      );
      heldStored = !!cached?.token;
      if (cached?.token && Number(cached.expiresAt) > Date.now()) {
        state.session = cached.token;
        state.adminSession = true;
        state.sessionExpires = Number(cached.expiresAt);
        // A cached token is provisional until the server resolves it. Network
        // and 5xx failures keep auth unknown; only an explicit 401 means absent.
        let who;
        const whoController = new AbortController();
        const whoTimeout = setTimeout(() => whoController.abort(), 2000);
        try {
          who = await fetch(server + "/whoami", {
            headers: { "x-session-token": state.session },
            signal: whoController.signal,
          });
        } catch (e) {
          return current();
        } finally {
          clearTimeout(whoTimeout);
        }
        if (!current()) return false;
        if (who.status === 401) {
          state.session = null;
          state.adminSession = false;
          state.sessionExpires = null;
          state.authStatus = "unauthenticated";
          localStorage.removeItem("loca-admin-session");
          sessionStorage.removeItem("loca-admin-session");
          if (!hostRenewed && await hostMasterSession(server, current)) {
            return current() && takeSessionFor(server, current, true);
          }
          return current();
        }
        if (!who.ok) return true;
        const identity = await who.json();
        if (!current()) return false;
        if (identity.name) {
          state.name = identity.name;
          $("name").value = state.name;
          cached.name = state.name;
        }
        localStorage.setItem("loca-admin-session", JSON.stringify(cached));
        sessionStorage.removeItem("loca-admin-session");
        state.authStatus = "authenticated";
        return true;
      }
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (e) {
      // A body that failed after its person left is not a reason to drop the
      // session the next person stored.
      if (!current()) return false;
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    }
    if (heldStored && !hostRenewed && await hostMasterSession(server, current)) {
      return current() && takeSessionFor(server, current, true);
    }
    if (!current()) return false;
    state.authStatus = "unauthenticated";
    return true;      // open house or no current credential
  }
  // Admin wins when both fields were filled. Mixing credentials made the room
  // token win server-side while leaving the root/bootstrap credential resident in the
  // browser; one request must have one unambiguous identity.
  const adminExchange = !!state.pairing;
  try {
    // Not cancelled when the generation moves (see the fetch wrapper above):
    // the server makes the session whether or not anyone is still here, and
    // only its token can end it. Every line below checks current() after each
    // wait, and a stale answer is applied to nothing and ended.
    const r = await pageFetch(server + "/sessions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(adminExchange ? { "x-pairing-code": state.pairing } : {}),
        ...(!adminExchange && state.roomToken ? { "x-room-token": state.roomToken } : {}),
      },
      body: JSON.stringify({ name: adminExchange ? "" : state.name, kind: "user" }),
    });
    if (!current()) {
      if (r.ok) endUnheldSession(server, (await r.json().catch(() => null))?.session_token);
      return false;
    }
    if (r.ok) {
      const info = await r.json();
      if (!current()) { endUnheldSession(server, info?.session_token); return false; }
      state.session = info.session_token || null;
      state.authStatus = state.session ? "authenticated" : "unauthenticated";
      state.adminSession = info.admin === true;
      state.sessionExpires = info.expires_at || null;
      // Credentials name the seat. Never keep a user-entered alias after the
      // server has returned the canonical session identity.
      if (info.name) {
        state.name = info.name;
        $("name").value = state.name;
      }
      // A successful pairing returns a server-confirmed admin
      // authority, so drop the raw key from memory and clear the input. From
      // here on admin rides on the session (header + WS), never the key.
      // PRINCIPLES: the key does not linger in the browser. If the session
      // expires the door asks for the key again — nothing is stored.
      if (adminExchange && state.session && state.adminSession) {
        try {
          localStorage.setItem("loca-admin-session", JSON.stringify({
            token: state.session, expiresAt: state.sessionExpires, name: state.name,
          }));
        } catch (e) {}
        state.pairing = "";
        state.roomToken = "";
        try { $("pairingCode").value = ""; } catch (e) {}
        try { $("roomToken").value = ""; } catch (e) {}
      }
    } else if (r.status === 401 || r.status === 403) {
      state.authStatus = "unauthenticated";
    }
  } catch (e) {
    // Retain unknown: transport failure is not an auth verdict. Overtaken, it
    // is not this page's failure at all.
    if (!current()) return false;
  }
  return true;
}
// Loca Host only (the desktop app with its own server): its page's Master
// session is minted by the shell from the Host's admin token, at start and,
// through this, again when it has ended (expired, or ended on the server).
// The page receives the bounded session only, never the token (main.rs
// host_master_session). Anywhere else (a browser, the desktop client) there
// is no Host to ask and this does nothing. Resolves true when a fresh session
// is stored for takeSession to use.
//
// It is asked only to replace a session this page held (takeSessionFor), and
// its answer is for whoever asked: if that person has signed out, someone else
// has signed in or the server changed while the Host was minting, the answer
// is not stored, and the session it carries is ended at the server, since
// nobody holds it.
function isLocaHostPage() {
  const status = window.__LOCA_HOST_PROVISIONING__;
  const tauri = window.__TAURI__;
  return !!status && status.state !== "not_applicable"
    && !!(tauri && tauri.core && typeof tauri.core.invoke === "function");
}
async function hostMasterSession(server, current) {
  if (!isLocaHostPage()) return false;
  try {
    const value = await window.__TAURI__.core.invoke("host_master_session");
    const minted = JSON.parse(value);
    if (!minted?.token || !(Number(minted.expiresAt) > Date.now())) return false;
    if (!current()) { endUnheldSession(server, minted.token); return false; }
    localStorage.setItem("loca-admin-session", value);
    return true;
  } catch (e) { return false; }
}
// The Host page's Master session was refused while in use: take the next one
// from the Host and carry on, the loca and the socket as they were. One at a
// time; if the Host cannot give one, the door closes as anywhere else.
let hostRenewal = null;
function renewHostSession() {
  if (hostRenewal) return hostRenewal;
  const gen = state.buildingGen;
  hostRenewal = (async () => {
    state.session = null;   // so takeSession asks again rather than keep the refused one
    if (!(await takeSession()) || buildingStale(gen)) return;
    if (state.authStatus === "authenticated" && state.session) {
      if (!steppedDown()) openWs();
      refreshRooms();
      return;
    }
    if (state.authStatus === "unauthenticated") lockRefusedSession();
  })().finally(() => { hostRenewal = null; });
  return hostRenewal;
}
// A session nobody at this tab holds any more stays valid on its server until
// it expires: an exchange answered after its person left, someone else took
// the tab or the server address changed; the session of a person who left;
// the session a new exchange replaces. End it at the server that granted it,
// with its own token and nothing else: never the current person's headers,
// never the current server. It is sent with the page's own fetch, so the next
// generation's cancel does not reach it, and kept alive past an unload. Its
// outcome is nobody's to see.
//
// This does not end every such session. A closed or crashed tab sends
// nothing; an answer that cannot be read (a broken connection, an unreadable
// body) never shows the token; a DELETE that fails is not retried. Those
// sessions stay on the server until it ends them: at their expiry, or, for an
// invite session, which has none, at a restart or when its key is revoked.
function endUnheldSession(server, token) {
  if (!token) return;
  pageFetch(server + "/sessions", {
    method: "DELETE", headers: { "x-session-token": token }, keepalive: true,
  }).catch(() => {});
}
// The Master session this browser keeps for its next load and its other tabs
// is still held, by the browser, even when this tab lets go of it.
function storedAdminSession(token) {
  try {
    const cached = JSON.parse(localStorage.getItem("loca-admin-session") || "null");
    return !!token && cached?.token === token;
  } catch (e) { return false; }
}
// Admin with a live master session, or when the server has no ADMIN_TOKEN
// configured (dev mode — open to everyone).
function isAdmin() { return state.adminSession === true || state.adminOpen === true; }
// Looking in from the door is not sitting at the table. A Master or Smaster
// can open every loca; opening one only WATCHES it (a `?watch=1` socket: the
// stream flows, no roster seat, nothing said). Taking a seat is an explicit,
// per-loca choice remembered in this browser. Care is the exception: it is
// the ranks' own house (PRINCIPLES: "Master and Smaster enter by rank"),
// so there they sit by default and stepping out is the remembered choice.
const SEATED_ROOMS_KEY = "loca-seated-rooms";
const DOOR_ROOMS_KEY = "loca-door-rooms";
function roomSet(key) {
  try {
    const list = JSON.parse(localStorage.getItem(key) || "[]");
    return new Set(Array.isArray(list) ? list.filter(r => typeof r === "string") : []);
  } catch (e) { return new Set(); }
}
function saveRoomSet(key, rooms) {
  try { localStorage.setItem(key, JSON.stringify([...rooms])); } catch (e) {}
}
function isRankHome(room) { return !!room && room === state.homeRoom; }
function setSeatedHere(room, seated) {
  const key = isRankHome(room) ? DOOR_ROOMS_KEY : SEATED_ROOMS_KEY;
  const rooms = roomSet(key);
  const listed = isRankHome(room) ? !seated : seated;
  if (listed) rooms.add(room); else rooms.delete(room);
  saveRoomSet(key, rooms);
}
function isWatchingDoor() {
  if (state.adminSession !== true || !state.room) return false;
  // A direct message has no door: its two sit, and nobody else is let near it.
  if (isDm(state.room)) return false;
  if (isRankHome(state.room)) return roomSet(DOOR_ROOMS_KEY).has(state.room);
  return !roomSet(SEATED_ROOMS_KEY).has(state.room);
}
// The door: locked until this client is actually a member of the server.
function setLocked(on) {
  if (state.authStatus === "unknown") return;
  document.body.classList.toggle("locked", on);
  if (on) { setConnOpen(true); $("whoami").classList.remove("on"); }
}

// The desktop Host's page talks only to the server its Host started, which
// the shell injects (main.rs page_server): never a typed address, the
// placeholder or the page's own origin. When that server did not start there
// is none (hostWithoutServer) and the page talks to nobody.
function hostWithoutServer() {
  return window.__LOCA_LOCK_SERVER__ === true && !window.__LOCA_DEFAULT_SERVER__;
}
function serverBase() {
  if (window.__LOCA_LOCK_SERVER__ === true) {
    return String(window.__LOCA_DEFAULT_SERVER__ || "").trim().replace(/\/+$/, "");
  }
  let s = $("server").value.trim() || $("server").placeholder;
  return s.replace(/\/+$/, "");
}
function wsBase() { return serverBase().replace(/^http/, "ws"); }

// The page's session is refused and no new one can be taken: back behind the
// door, nothing of the room shown, and the socket closed rather than retried
// with a session that will not come back.
function lockRefusedSession() {
  // A page that held no session (a first visit) has nothing that ended.
  const held = !!state.session || state.adminSession;
  if (state.adminSession) {
    try {
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (e) {}
    state.session = null;
    state.adminSession = false;
    state.sessionExpires = null;
    state.authStatus = "unauthenticated";
  }
  if (state.ws) { state.ws.onclose = null; state.ws.close(); state.ws = null; }
  setLocked(true);
  clearBuildingData();
  clearPersonWords();
  setStatus("locked — key required", false);
  // A reason the desktop shell gave for starting this page signed out (its
  // sign-in could not be kept across a reload: main.rs KEYCHAIN_SHIM) stays
  // on the sign-in view until the person signs in again.
  if (window.__LOCA_SIGNIN_NOTICE__) {
    $("doorline").textContent = String(window.__LOCA_SIGNIN_NOTICE__);
    return;
  }
  // With a key, the key itself was just refused; without one (a Master
  // pairing, which is used once) the session ended and a new code is needed.
  $("doorline").innerHTML = state.roomToken
    ? 'that key does not open this table.'
    : held ? 'your session ended.<br>enter a fresh master pairing code or loca key.'
    : 'a private table for people and agents.<br>sign in with your key, a loca invite or a Master pairing code.';
}

// The page's current session is no longer accepted (it expired, or was
// ended), and the page holds a re-usable key (an invite, a membership key, the
// sandbox room key): take a new session with it and carry on, the loca and
// whatever was typed left as they are, the socket reopened. Every trigger
// (the poll's 401, a socket the server closed) joins the one renewal out.
// Only the page's current session is renewed, never one replaced since, and
// a key the server refuses locks the door once: there is no second try.
// Resolves "renewed", "refused" or "" (nothing to do, overtaken, or no
// verdict from the network: the health poll asks again, retryOwedRenewal).
//
// A renewal belongs to the person (the generation), the server and the key it
// was started for. One out for an earlier person is never joined by, never
// blocks and never applies to whoever is here now: they get their own.
function renewalKey() { return JSON.stringify([state.buildingGen, serverBase(), state.roomToken]); }
let sessionRenewal = null;   // { key, promise } while a renewal is out
let owedRenewal = "";        // the key of a renewal the network left unanswered
function renewRefusedSession(refused) {
  const key = renewalKey();
  if (sessionRenewal && sessionRenewal.key === key) return sessionRenewal.promise;
  if (!state.roomToken || state.pairing || state.session !== refused) return Promise.resolve("");
  // No session at all: only when this key's last renewal got no answer.
  if (!refused && owedRenewal !== key) return Promise.resolve("");
  const gen = state.buildingGen;
  const renewal = { key, promise: null };
  renewal.promise = (async () => {
    // The refused session is being replaced: end it (the server has said it
    // is gone; this only makes sure).
    endUnheldSession(state.server || serverBase(), refused);
    if (!(await takeSession()) || buildingStale(gen)) return "";
    if (state.authStatus === "authenticated" && state.session) {
      owedRenewal = "";
      // A tab that stepped down for a newer connection (onEvicted) stays
      // out: the new session does not take the seat back by itself.
      if (!steppedDown()) openWs();
      return "renewed";
    }
    if (state.authStatus === "unauthenticated") {
      owedRenewal = "";
      lockRefusedSession();
      return "refused";
    }
    // No verdict (the network): still this person's, still reconnecting.
    owedRenewal = key;
    setStatus("reconnecting…", false);
    return "";
  })().finally(() => { if (sessionRenewal === renewal) sessionRenewal = null; });
  sessionRenewal = renewal;
  return renewal.promise;
}
// A renewal the network left without a verdict is tried again by the health
// poll on its own: once per tick at most, never two at once, and only for the
// same person and key. (The poll's /rooms read may well succeed with the raw
// key meanwhile, so a 401 cannot be waited for.)
function retryOwedRenewal() {
  if (owedRenewal && owedRenewal === renewalKey() && !state.session && !sessionTaking) {
    renewRefusedSession(null);
  }
}

let roomsRefreshing = false;   // false, or the generation whose refresh is under way
async function refreshRooms() {
  const gen = state.buildingGen;
  const session = state.session;
  if (roomsRefreshing === gen) return;
  roomsRefreshing = gen;
  try {
    const r = await fetch(serverBase() + "/rooms", { headers: adminHeaders({}) });
    // An answer to a request from before a sign-out or sign-in says nothing
    // about whoever is here now: a late 401 must not close their session.
    if (buildingStale(gen) || session !== state.session) return;
    if (r.status === 401) {
      // The session ended (it expired, or was ended), not necessarily the
      // key: a page that holds a re-usable key takes a new session and
      // carries on. Only a refused key, or no key, closes the door.
      if (state.roomToken && !state.pairing) { await renewRefusedSession(session); return; }
      if (state.authStatus === "unknown") return;
      if (state.adminSession && !state.pairing && isLocaHostPage()) { await renewHostSession(); return; }
      lockRefusedSession();
      return;
    }
    setLocked(false);
    const rooms = await r.json();
    if (buildingStale(gen)) return;
    ensureReadCursors();
    await updateUnreadCounts(rooms);
    if (buildingStale(gen)) return;
    // Direct messages live under Messages, never in the loca list.
    state.rooms = rooms.filter(rm => !isDm(rm.room));
    await fetchDms();
    renderRooms();
    renderLocaSidebar();
  } catch (e) {
    // Cancelled because its person left: nothing to report to the next one.
    if (!buildingStale(gen)) setStatus("cannot reach server", false);
  } finally {
    if (roomsRefreshing === gen) roomsRefreshing = false;
  }
}

// Floors. The interface calls a group a floor and the default group the
// Ground floor (PRINCIPLES "Groups"; docs/concepts.md). A loca's group comes
// only from the room list the server gives this viewer (RoomSummary.group,
// group_id: named only to those who may enter the loca), so a floor is drawn
// only with a loca this viewer is listed. Nothing else is asked for.
const GROUND_FLOOR = "";   // the floor key of a loca with no group
function roomFloorKey(rm) { return rm && rm.group ? (rm.group_id || rm.group) : GROUND_FLOOR; }
// A loca just navigated to: its floor opens when it is next drawn.
let floorRevealFor = null;
function revealRoomFloor(room) { floorRevealFor = room; }

// A floor's heading reads as a floor: "Product floor", beside "Ground floor".
// Only the heading says so; the group keeps its own name everywhere else. A
// name that already ends in "floor" is not given the word twice.
function floorSaysFloor(floor) {
  return floor.key !== GROUND_FLOOR && !/floor\s*$/i.test(floor.name || "");
}

function renderRooms() {
  const list = $("roomList");
  list.innerHTML = "";
  if (floorRevealFor) {
    const rm = state.rooms.find(r => r.room === floorRevealFor);
    if (rm) { floorRevealFor = null; setFloorCollapsed(roomFloorKey(rm), false); }
  }
  const prefs = loadRoomPreferences();
  const ordered = orderedSidebarRooms(state.rooms);
  const floors = new Map([[GROUND_FLOOR, { key: GROUND_FLOOR, name: "Ground floor", rooms: [] }]]);
  for (const rm of ordered) {
    const key = roomFloorKey(rm);
    if (!floors.has(key)) floors.set(key, { key, name: rm.group, rooms: [] });
    floors.get(key).rooms.push(rm);
  }
  const roomItem = (rm) => {
    const unread = Number(state.unread[rm.room] || 0);
    const el = document.createElement("button");
    el.type = "button";
    el.className = "room" + (rm.special ? " special" : "") +
      (rm.archived ? " archived" : "") +
      (unread ? " has-unread" : "") +
      (rm.room === state.room ? " active" : "");
    el.title = locaRowTitle(rm);
    el.setAttribute("aria-current", rm.room === state.room ? "page" : "false");
    const people = Number(rm.humans || 0);
    const agents = Number(rm.agents || 0);
    const presenceLabel = [
      people ? `${people} ${people === 1 ? "person" : "people"}` : "",
      agents ? `${agents} ${agents === 1 ? "agent" : "agents"}` : "",
    ].filter(Boolean).join(" · ");
    const pres = (people || agents)
      ? `<span class="pres" title="${esc(presenceLabel)}">${esc(presenceLabel)}</span>`
      : `<span class="pres" style="color:var(--dim)">empty</span>`;
    const badge = unread
      ? `<span class="unreadbadge" title="${unread} unread message${unread === 1 ? "" : "s"}">${unread > 99 ? "99+" : unread}</span>`
      : "";
    // Care (the Building's own loca) has no floor of its own, so like every
    // loca without one it is on the Ground floor. The tag says what sets it
    // apart there: only the ranks and the caretakers may enter it.
    const care = isRankHome(rm.room)
      ? `<span class="caretag" title="The Building's care loca: the Master, Smasters and caretakers only">care</span>`
      : "";
    el.innerHTML = `<div class="rrow">${locaMarkHtml(rm)}<span class="rname" title="${esc(locaRowTitle(rm))}">${esc(rm.room)}</span>${care}${pres}</div>` +
      (rm.last || badge ? `<div class="rlastrow"><div class="rlast">${esc(rm.last || "")}</div>${badge}</div>` : "");
    // The room list is only visible after identity resolution. Moving
    // between locas is navigation, not a new login: re-reading an autofilled
    // or already-spent credential here could replace a healthy session with
    // the login door until refresh.
    el.onclick = () => { setMobileSidebar(false); joinRoom(rm.room); };
    const item = document.createElement("div");
    item.className = "roomitem";
    item.append(el, renderRoomPreferenceActions(rm.room));
    return item;
  };
  const ground = floors.get(GROUND_FLOOR);
  const named = [...floors.values()].filter(f => f.key !== GROUND_FLOOR)
    .sort((a, b) => a.name.localeCompare(b.name));
  // Every loca is drawn under its floor's heading, the Ground floor's
  // included even when it is the only floor: the list has no other title
  // (the owner's rule: a loca without a floor is on the Ground floor, and the
  // sidebar says so).
  {
    // Every loca without a group is on the Ground floor, Care included (the
    // owner's rule: no exception). It is listed first there.
    const home = ground.rooms.find(rm => isRankHome(rm.room));
    if (home) ground.rooms = [home, ...ground.rooms.filter(rm => rm !== home)];
    for (const floor of [ground, ...named]) {
      if (!floor.rooms.length) continue;   // no loca of it listed here
      const collapsed = prefs.collapsed.includes(floor.key);
      const details = document.createElement("details");
      details.className = "floor";
      details.dataset.floor = floor.key;
      details.open = !collapsed;
      const head = document.createElement("summary");
      head.className = "rgrouphead" + (floorSaysFloor(floor) ? " saysfloor" : "");
      const unread = floor.rooms.reduce((n, rm) => n + Number(state.unread[rm.room] || 0), 0);
      const badge = collapsed && unread
        ? `<span class="unreadbadge" title="${unread} unread message${unread === 1 ? "" : "s"} on this floor">${unread > 99 ? "99+" : unread}</span>`
        : "";
      head.innerHTML = `${typeof GROUP_ICON === "string" ? GROUP_ICON : ""}` +
        `<span class="rgroupname" title="${esc(floor.name)}">${esc(floor.name)}</span>${badge}` +
        `<span class="rgroupcount" aria-label="${floor.rooms.length} locas">${floor.rooms.length}</span>`;
      // The loca one is in stays in sight under a closed floor (drawn just
      // after it, below).
      const here = collapsed ? floor.rooms.find(rm => rm.room === state.room) : null;
      details.append(head, ...floor.rooms.filter(rm => rm !== here).map(roomItem));
      // Open or closed is this viewer's own (kept with the sidebar order).
      details.addEventListener("toggle", () => {
        if (details.open === !loadRoomPreferences().collapsed.includes(floor.key)) return;
        const focused = document.activeElement === head;
        setFloorCollapsed(floor.key, !details.open);
        renderRooms();
        if (focused) [...$("roomList").querySelectorAll("details.floor")]
          .find(d => d.dataset.floor === floor.key)?.querySelector("summary")?.focus();
      });
      list.appendChild(details);
      if (here) {
        const kept = roomItem(here);
        kept.classList.add("floorkept");
        list.appendChild(kept);
      }
      // A closed floor says so in words: without this line the one loca kept
      // in sight reads as the whole floor.
      const hidden = collapsed ? floor.rooms.length - (here ? 1 : 0) : 0;
      if (hidden) {
        const more = document.createElement("button");
        more.type = "button";
        more.className = "floormore";
        more.dataset.floorMore = floor.key;
        more.textContent = here ? `+ ${hidden} more on this floor` : `Show ${hidden} loca${hidden === 1 ? "" : "s"}`;
        more.addEventListener("click", () => { setFloorCollapsed(floor.key, false); renderRooms(); });
        list.appendChild(more);
      }
    }
  }
  renderHiddenRooms();
  renderDmList();
}

function ensureReadCursors() {
  const key = `loca-read:${serverBase()}:${state.name}`;
  if (state.readStorageKey === key) return;
  state.readStorageKey = key;
  state.unread = {};
  state.roomLatest = {};
  state.unreadChecked = {};
  try {
    state.readCursors = JSON.parse(localStorage.getItem(key) || "{}") || {};
  } catch (e) {
    state.readCursors = {};
  }
}

function saveReadCursors() {
  try { localStorage.setItem(state.readStorageKey, JSON.stringify(state.readCursors)); }
  catch (e) {}
}

function markRoomRead(room, through) {
  if (!room) return;
  ensureReadCursors();
  const latest = Math.max(Number(through || 0), Number(state.roomLatest[room] || 0));
  state.readCursors[room] = latest;
  state.roomLatest[room] = latest;
  state.unreadChecked[room] = latest;
  state.unread[room] = 0;
  saveReadCursors();
  renderRooms();
}

async function updateUnreadCounts(rooms) {
  let cursorChanged = false;
  // This browser already knew this person's rooms before this refresh.
  const known = Object.keys(state.readCursors).length > 0;
  await Promise.all(rooms.map(async (rm) => {
    const room = rm.room;
    const latest = Number(rm.last_id || 0);
    state.roomLatest[room] = latest;
    if (!(room in state.readCursors) && known && isDm(room)) {
      // A conversation that appears now was just opened: everything said in
      // it is new, the first word included, and is counted below.
      state.readCursors[room] = 0;
      cursorChanged = true;
    }
    if (!(room in state.readCursors)) {
      // First sight establishes a baseline. Old history is not "new".
      state.readCursors[room] = latest;
      state.unreadChecked[room] = latest;
      state.unread[room] = 0;
      cursorChanged = true;
      return;
    }
    if (room === state.room && state.tab === "chat" && !document.hidden) {
      if (Number(state.readCursors[room] || 0) !== latest) cursorChanged = true;
      state.readCursors[room] = latest;
      state.unreadChecked[room] = latest;
      state.unread[room] = 0;
      return;
    }
    const read = Number(state.readCursors[room] || 0);
    if (latest < read) {
      // A memory-only server restart can reset message ids. Its epoch reloads
      // the page, then this rollback establishes the new generation's baseline.
      state.readCursors[room] = latest;
      state.unread[room] = 0;
      state.unreadChecked[room] = latest;
      cursorChanged = true;
      return;
    }
    if (latest === read) {
      state.unread[room] = 0;
      state.unreadChecked[room] = latest;
      return;
    }
    if (Number(state.unreadChecked[room] || 0) === latest) return;
    try {
      const tail = await fetch(
        `${serverBase()}/rooms/${encodeURIComponent(room)}/messages?since=${read}`,
        { headers: adminHeaders({}) }
      );
      if (!tail.ok) return;
      const messages = await tail.json();
      // The room was read while this count was on its way (opened, or a
      // conversation closed): it counts from the old read point. Applying it
      // would mark words unread that were already read, and a closed
      // conversation would come back without anything new being said. The
      // next refresh counts again from where reading now stands.
      if (Number(state.readCursors[room] || 0) !== read) return;
      state.unread[room] = messages.filter(m => m.sender !== state.name).length;
      state.unreadChecked[room] = latest;
    } catch (e) {}
  }));
  if (cursorChanged) saveReadCursors();
}
