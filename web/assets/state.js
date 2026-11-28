"use strict";
// Shared browser state, identity, room navigation, and unread cursors.
const $ = (id) => document.getElementById(id);
const state = { server: "", name: "operator", room: null, rooms: [], ws: null, members: [], lobby: [], tab: "chat", sidebarView: "building", locaOperator: null, locaContext: null, principalId: null, roomPreferences: { pinned: [], hidden: [], order: [], collapsed: [] }, notes: {}, editing: null, pairing: "", roomToken: "", session: null, authStatus: "unknown", adminSession: false, sessionExpires: null, profile: null, credentials: [], epoch: null, homeRoom: "iye", locaAgents: [], mode: { mode: "free" }, settings: { rate_limit: 10, rate_window_secs: 30 }, mod: { muted: [], banned: [] }, tasks: {}, goals: {}, attentions: {}, waits: {}, journal: [], lastId: 0, seen: new Set(), msgs: [], replyTo: null, reminderReceipts: new Set(), unread: {}, readCursors: {}, roomLatest: {}, unreadChecked: {}, readStorageKey: "", pendingAttachments: [], pinned: null, pinnedExpanded: false };
Object.assign(state, { memory: null, memoryEntries: [], memoryStatus: "loading", memoryError: "" });
// Of each loca's unread messages, how many name this person (@name or a
// direct address). Counted from the same unread tail as the count itself;
// Home (home.js) shows them.
state.unreadMentions = {};
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
  // A mentat room has no seat for a rank: it watches and manages from the
  // door, and writes only in its own turn in the line (mentat.js).
  if (state.settings?.mentat_room) return true;
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

let roomsRefreshing = false;   // false, or the generation whose refresh is under way
// A refresh asked for while one is under way is not dropped: the one under
// way may have started before the page knew who is here (its answer is then
// set aside), so it runs once more when that one ends.
let roomsRefreshAgain = false;
async function refreshRooms() {
  const gen = state.buildingGen;
  const session = state.session;
  if (roomsRefreshing === gen) { roomsRefreshAgain = true; return; }
  roomsRefreshing = gen;
  roomsRefreshAgain = false;
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
    // Direct messages live under Messages, never in the loca list; the
    // Lobby room has its own place at the top of the sidebar.
    state.lobbyRoom = rooms.find(rm => isLobbyRoom(rm.room)) || null;
    state.rooms = rooms.filter(rm => !isDm(rm.room) && !isLobbyRoom(rm.room));
    await fetchDms();
    renderRooms();
    renderLocaSidebar();
  } catch (e) {
    // Cancelled because its person left: nothing to report to the next one.
    if (buildingStale(gen)) return;
    setStatus("cannot reach server", false);
    // Nobody is signed in, so there is no room to show and the one thing to
    // do is sign in, perhaps at another address: the sign-in page, saying
    // which address did not answer. A signed-in page keeps its room and
    // retries (the socket and the health poll do).
    // An open house (a server with no Master key, development only) needs no
    // sign-in, so a moment without it is not a reason to show the door.
    if (state.authStatus === "unauthenticated" && session === state.session && state.adminOpen !== true) showServerUnreachable();
  } finally {
    if (roomsRefreshing === gen) roomsRefreshing = false;
    if (roomsRefreshAgain && !buildingStale(gen)) { roomsRefreshAgain = false; refreshRooms(); }
  }
}

function showServerUnreachable() {
  setLocked(true);
  // The address is the thing to change: open the form's Advanced part.
  const advanced = $("serverAdvanced");
  if (advanced) advanced.open = true;
  $("doorline").textContent = `cannot reach ${serverBase()} — check the server address.`;
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

function renderLobbyRoomLink() {
  document.querySelector("main.main")?.classList.toggle("lobbyroom", isLobbyRoom(state.room));
  // This Loca can stand outside main (beside the chat): the page says it too.
  document.body.classList.toggle("in-lobbyroom", isLobbyRoom(state.room));
  const link = $("lobbyRoomLink");
  if (!link) return;
  const lobby = state.lobbyRoom;
  link.classList.toggle("hidden", !lobby);
  if (!lobby) return;
  const here = Number(lobby.humans || 0) + Number(lobby.agents || 0);
  $("lobbyRoomCount").textContent = here ? `${here} here` : "";
  const active = isLobbyRoom(state.room);
  link.classList.toggle("active", active);
  link.setAttribute("aria-current", active ? "page" : "false");
  const unread = Number(state.unread[LOBBY_ROOM] || 0);
  link.classList.toggle("has-unread", !!unread);
}

function renderRooms() {
  renderLobbyRoomLink();
  const list = $("roomList");
  list.innerHTML = "";
  if (floorRevealFor) {
    const rm = state.rooms.find(r => r.room === floorRevealFor);
    if (rm) { floorRevealFor = null; setFloorCollapsed(roomFloorKey(rm), false); }
  }
  const prefs = loadRoomPreferences();
  // The sidebar's search (sidebar.js) narrows the list to the locas whose
  // name holds what is typed; a floor with none of them is left out.
  const searching = sidebarFilterActive();
  const ordered = orderedSidebarRooms(state.rooms).filter(rm => sidebarMatches(locaName(rm.room), rm.room));
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
    el.innerHTML = `<div class="rrow">${locaMarkHtml(rm)}<span class="rname" title="${esc(locaRowTitle(rm))}">${esc(locaName(rm.room))}</span>${care}${pres}</div>` +
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
      // While searching, a floor with a match is drawn open; closing it
      // then still counts for after the search.
      const collapsed = prefs.collapsed.includes(floor.key) && !searching;
      const details = document.createElement("details");
      details.className = "floor";
      details.dataset.floor = floor.key;
      details.open = !collapsed;
      const head = document.createElement("summary");
      head.className = "rgrouphead" + (floorSaysFloor(floor) ? " saysfloor" : "");
      const colour = floor.key !== GROUND_FLOOR && typeof floorColour === "function" ? floorColour(floor.key) : "";
      if (colour) { head.classList.add("floorc"); head.style.setProperty("--floor", colour); }
      const unread = floor.rooms.reduce((n, rm) => n + Number(state.unread[rm.room] || 0), 0);
      const badge = collapsed && unread
        ? `<span class="unreadbadge" title="${unread} unread message${unread === 1 ? "" : "s"} on this floor">${unread > 99 ? "99+" : unread}</span>`
        : "";
      head.innerHTML = `${typeof GROUP_ICON === "string" ? GROUP_ICON : ""}` +
        `<span class="rgroupname" title="${esc(floor.name)}">${esc(floor.name)}</span>${badge}` +
        `<span class="rgroupcount" aria-label="${floor.rooms.length} locas">${floor.rooms.length}</span>`;
      // A + at the heading's end: a new loca on this floor (sidebar.js).
      if (floorAcceptsNewLoca(floor.key)) head.append(floorNewLocaButton(floor));
      // A closed floor is closed: its heading, its count and its unread
      // badge, and no loca of it (the owner's rule: no row left standing).
      details.append(head, ...floor.rooms.map(roomItem));
      // Open or closed is this viewer's own (kept with the sidebar order).
      // Only a toggle made on this heading counts. A <details> also fires
      // `toggle` for the state it was drawn in, later; by then the viewer's
      // preferences may have loaded or the list been redrawn, and comparing
      // with them wrote "open" over a stored close.
      const drawnOpen = details.open;
      details.addEventListener("toggle", () => {
        if (details.open === drawnOpen || !details.isConnected) return;
        const focused = document.activeElement === head;
        setFloorCollapsed(floor.key, !details.open);
        renderRooms();
        if (focused) [...$("roomList").querySelectorAll("details.floor")]
          .find(d => d.dataset.floor === floor.key)?.querySelector("summary")?.focus();
      });
      list.appendChild(details);
    }
  }
  if (searching && !list.children.length) {
    list.innerHTML = `<div class="sidesearchnone">No loca matches “${esc(sidebarQueryText())}”.</div>`;
  }
  renderHiddenRooms();
  renderDmList();
  // Home shows the same summaries, counts and last lines (home.js).
  if (typeof refreshHome === "function") refreshHome();
}

function ensureReadCursors() {
  const key = `loca-read:${serverBase()}:${state.name}`;
  if (state.readStorageKey === key) return;
  state.readStorageKey = key;
  state.unread = {};
  state.unreadMentions = {};
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
  state.unreadMentions[room] = 0;
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
      soundForTail(room, messages, state.unreadChecked[room]);
      state.unread[room] = messages.filter(m => m.sender !== state.name).length;
      state.unreadMentions[room] = messages.filter(m => isMentionOfMe(m, state.name)).length;
      state.unreadChecked[room] = latest;
    } catch (e) {}
  }));
  if (cursorChanged) saveReadCursors();
}
