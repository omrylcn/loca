"use strict";
// Shared browser state, identity, room navigation, and unread cursors.
const $ = (id) => document.getElementById(id);
const state = { server: "", name: "operator", room: null, rooms: [], ws: null, members: [], lobby: [], tab: "chat", sidebarView: "building", locaOperator: null, locaContext: null, principalId: null, roomPreferences: { pinned: [], hidden: [], order: [] }, notes: {}, editing: null, pairing: "", roomToken: "", session: null, authStatus: "unknown", adminSession: false, sessionExpires: null, profile: null, credentials: [], epoch: null, homeRoom: "iye", locaAgents: [], mode: { mode: "free" }, settings: { rate_limit: 10, rate_window_secs: 30 }, mod: { muted: [], banned: [] }, tasks: {}, goals: {}, attentions: {}, waits: {}, journal: [], lastId: 0, seen: new Set(), msgs: [], replyTo: null, reminderReceipts: new Set(), unread: {}, readCursors: {}, roomLatest: {}, unreadChecked: {}, readStorageKey: "", pendingAttachments: [], pinned: null, pinnedExpanded: false };
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

function renderHostProvisioningStatus() {
  const box = $("hostProvisionError");
  if (!box) return;
  const status = window.__LOCA_HOST_PROVISIONING__;
  const failed = status && status.state === "error";
  box.textContent = failed
    ? "Master session could not be prepared. Building administration is unavailable. Restart Loca Host to retry."
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
// sessions persist across deploys until their selected expiry; davet sessions
// can be renewed from their local davet after a restart.
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
async function takeSessionFor(server, current) {
  state.authStatus = "unknown";
  state.session = null;
  state.adminSession = false;
  state.sessionExpires = null;
  if (!state.roomToken && !state.pairing) {
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
          return true;
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
    state.authStatus = "unauthenticated";
    return true;      // open house or no current credential
  }
  // Admin wins when both fields were filled. Mixing credentials made the room
  // token win server-side while leaving the root/bootstrap credential resident in the
  // browser; one request must have one unambiguous identity.
  const adminExchange = !!state.pairing;
  try {
    const r = await fetch(server + "/sessions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(adminExchange ? { "x-pairing-code": state.pairing } : {}),
        ...(!adminExchange && state.roomToken ? { "x-room-token": state.roomToken } : {}),
      },
      body: JSON.stringify({ name: adminExchange ? "" : state.name, kind: "user" }),
    });
    if (!current()) return false;
    if (r.ok) {
      const info = await r.json();
      if (!current()) return false;
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
// Admin with a live master session, or when the server has no ADMIN_TOKEN
// configured (dev mode — open to everyone).
function isAdmin() { return state.adminSession === true || state.adminOpen === true; }
// Looking in from the door is not sitting at the table. A Master or Smaster
// can open every loca; opening one only WATCHES it (a `?watch=1` socket: the
// stream flows, no roster seat, nothing said). Taking a seat is an explicit,
// per-loca choice remembered in this browser. İye is the exception: it is
// the ranks' own house (PRINCIPLES: "Master ve Smaster rütbeleriyle girer"),
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
  // A birebir has no door: its two sit, and nobody else is let near it.
  if (isBirebir(state.room)) return false;
  if (isRankHome(state.room)) return roomSet(DOOR_ROOMS_KEY).has(state.room);
  return !roomSet(SEATED_ROOMS_KEY).has(state.room);
}
// The door: locked until this client is actually a member of the server.
function setLocked(on) {
  if (state.authStatus === "unknown") return;
  document.body.classList.toggle("locked", on);
  if (on) { setConnOpen(true); $("whoami").classList.remove("on"); }
}

function serverBase() {
  let s = $("server").value.trim() || $("server").placeholder;
  return s.replace(/\/+$/, "");
}
function wsBase() { return serverBase().replace(/^http/, "ws"); }

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
      if (state.authStatus === "unknown") return;
      // Wrong or missing key: back behind the door, nothing of the room shown.
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
      setLocked(true);
      clearBuildingData();
      setStatus("locked — key required", false);
      $("doorline").innerHTML = state.roomToken
        ? 'that key does not open this table.'
        : 'your session ended.<br>enter a fresh master pairing code or loca key.';
      return;
    }
    setLocked(false);
    const rooms = await r.json();
    if (buildingStale(gen)) return;
    if (!rooms.length) rooms.push({ room: state.homeRoom, members: 0 });
    ensureReadCursors();
    await updateUnreadCounts(rooms);
    if (buildingStale(gen)) return;
    // Birebirs live under Messages, never in the loca list.
    state.rooms = rooms.filter(rm => !isBirebir(rm.room));
    await fetchBirebirs();
    renderRooms();
    renderLocaSidebar();
  } catch (e) {
    setStatus("cannot reach server", false);
  } finally {
    if (roomsRefreshing === gen) roomsRefreshing = false;
  }
}

function renderRooms() {
  const list = $("roomList");
  list.innerHTML = "";
  // Default-group locas first, exactly as before; each group's locas follow
  // under a header with the group's name.
  const ordered = orderedSidebarRooms(state.rooms);
  const sections = new Map();
  for (const rm of ordered) {
    if (!rm.group) continue;
    const key = rm.group_id || rm.group;
    if (!sections.has(key)) sections.set(key, { name: rm.group, rooms: [] });
    sections.get(key).rooms.push(rm);
  }
  const appendRoom = (rm) => {
    const unread = Number(state.unread[rm.room] || 0);
    const el = document.createElement("button");
    el.type = "button";
    el.className = "room" + (rm.special ? " special" : "") +
      (rm.archived ? " archived" : "") +
      (unread ? " has-unread" : "") +
      (rm.room === state.room ? " active" : "");
    el.title = rm.archived ? "closed · read-only" : "";
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
    el.innerHTML = `<div class="rrow"><span class="rname" title="${esc(rm.room)}">${esc(rm.room)}</span>${pres}</div>` +
      (rm.last || badge ? `<div class="rlastrow"><div class="rlast">${esc(rm.last || "")}</div>${badge}</div>` : "");
    // The room list is only visible after identity resolution. Moving
    // between locas is navigation, not a new login: re-reading an autofilled
    // or already-spent credential here could replace a healthy session with
    // the login door until refresh.
    el.onclick = () => { setMobileSidebar(false); joinRoom(rm.room); };
    const item = document.createElement("div");
    item.className = "roomitem";
    item.append(el, renderRoomPreferenceActions(rm.room));
    list.appendChild(item);
  };
  ordered.filter(rm => !rm.group).forEach(appendRoom);
  const groups = [...sections.values()].sort((a, b) => a.name.localeCompare(b.name));
  for (const section of groups) {
    const head = document.createElement("div");
    head.className = "rgrouphead";
    head.innerHTML = `${typeof GROUP_ICON === "string" ? GROUP_ICON : ""}<span class="rgroupname" title="${esc(section.name)}">${esc(section.name)}</span>` +
      `<span class="rgroupcount" aria-label="${section.rooms.length} locas">${section.rooms.length}</span>`;
    list.appendChild(head);
    section.rooms.forEach(appendRoom);
  }
  renderHiddenRooms();
  renderBirebirList();
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
  await Promise.all(rooms.map(async (rm) => {
    const room = rm.room;
    const latest = Number(rm.last_id || 0);
    state.roomLatest[room] = latest;
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
