"use strict";
// Shared browser state, identity, room navigation, and unread cursors.
const $ = (id) => document.getElementById(id);
const state = { server: "", name: "operator", room: null, rooms: [], ws: null, members: [], lobby: [], tab: "chat", sidebarView: "building", locaOperator: null, locaContext: null, principalId: null, roomPreferences: { pinned: [], hidden: [], order: [] }, notes: {}, editing: null, pairing: "", roomToken: "", session: null, adminSession: false, sessionExpires: null, profile: null, credentials: [], epoch: null, homeRoom: "general", locaAgents: [], mode: { mode: "free" }, settings: { rate_limit: 10, rate_window_secs: 30 }, mod: { muted: [], banned: [] }, tasks: {}, goals: {}, attentions: {}, waits: {}, journal: [], lastId: 0, seen: new Set(), msgs: [], replyTo: null, reminderReceipts: new Set(), unread: {}, readCursors: {}, roomLatest: {}, unreadChecked: {}, readStorageKey: "" };

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
async function takeSession() {
  // Room navigation is not a new login. Keep the already-bound identity;
  // clearing it here made every loca click throw the master back to the door.
  if (!state.roomToken && !state.pairing && state.session) return;
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
        if (cached.name) {
          state.name = cached.name;
          $("name").value = state.name;
        } else {
          // Compatibility for sessions cached before canonical names were
          // persisted. The session itself is the identity authority.
          try {
            const who = await fetch(serverBase() + "/whoami", {
              headers: { "x-session-token": state.session },
            });
            if (who.ok) {
              const identity = await who.json();
              if (identity.name) {
                state.name = identity.name;
                $("name").value = state.name;
                cached.name = state.name;
              }
            }
          } catch (e) {}
        }
        localStorage.setItem("loca-admin-session", JSON.stringify(cached));
        sessionStorage.removeItem("loca-admin-session");
        return;
      }
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (e) {
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    }
    return;      // open house or no current credential
  }
  // Admin wins when both fields were filled. Mixing credentials made the room
  // token win server-side while leaving the root/bootstrap credential resident in the
  // browser; one request must have one unambiguous identity.
  const adminExchange = !!state.pairing;
  try {
    const r = await fetch(serverBase() + "/sessions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(adminExchange ? { "x-pairing-code": state.pairing } : {}),
        ...(!adminExchange && state.roomToken ? { "x-room-token": state.roomToken } : {}),
      },
      body: JSON.stringify({ name: state.name, kind: "user" }),
    });
    if (r.ok) {
      const info = await r.json();
      state.session = info.session_token || null;
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
    }
  } catch (e) { /* stay tokenless; reads will say so */ }
}
// Admin with a live master session, or when the server has no ADMIN_TOKEN
// configured (dev mode — open to everyone).
function isAdmin() { return state.adminSession === true || state.adminOpen === true; }
// The door: locked until this client is actually a member of the server.
function setLocked(on) {
  document.body.classList.toggle("locked", on);
  if (on) { setConnOpen(true); $("whoami").classList.remove("on"); }
}

function serverBase() {
  let s = $("server").value.trim() || $("server").placeholder;
  return s.replace(/\/+$/, "");
}
function wsBase() { return serverBase().replace(/^http/, "ws"); }

let roomsRefreshing = false;
async function refreshRooms() {
  if (roomsRefreshing) return;
  roomsRefreshing = true;
  try {
    const r = await fetch(serverBase() + "/rooms", { headers: adminHeaders({}) });
    if (r.status === 401) {
      // Wrong or missing key: back behind the door, nothing of the room shown.
      if (state.adminSession) {
        try {
          localStorage.removeItem("loca-admin-session");
          sessionStorage.removeItem("loca-admin-session");
        } catch (e) {}
        state.session = null;
        state.adminSession = false;
        state.sessionExpires = null;
      }
      setLocked(true);
      setStatus("locked — key required", false);
      $("doorline").innerHTML = state.roomToken
        ? 'that key does not open this table.'
        : 'your session ended.<br>enter a fresh master pairing code or loca key.';
      return;
    }
    setLocked(false);
    const rooms = await r.json();
    if (!rooms.length) rooms.push({ room: state.homeRoom, members: 0 });
    ensureReadCursors();
    await updateUnreadCounts(rooms);
    state.rooms = rooms;
    renderRooms();
    renderLocaSidebar();
  } catch (e) {
    setStatus("cannot reach server", false);
  } finally {
    roomsRefreshing = false;
  }
}

function renderRooms() {
  const list = $("roomList");
  list.innerHTML = "";
  for (const rm of orderedSidebarRooms(state.rooms)) {
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
      el.innerHTML = `<div class="rrow"><span class="rname">${esc(rm.room)}</span>${pres}</div>` +
        (rm.last || badge ? `<div class="rlastrow"><div class="rlast">${esc(rm.last || "")}</div>${badge}</div>` : "");
      // Clicking a room IS connecting: identity is read from the inputs every
      // time, so joining before/without pressing connect behaves identically.
      el.onclick = () => doConnect(rm.room);
      const item = document.createElement("div");
      item.className = "roomitem";
      item.append(el, renderRoomPreferenceActions(rm.room));
      list.appendChild(item);
  }
  renderHiddenRooms();
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
      state.unread[room] = messages.filter(m => m.sender !== state.name).length;
      state.unreadChecked[room] = latest;
    } catch (e) {}
  }));
  if (cursorChanged) saveReadCursors();
}
