"use strict";
// Home (the Home and HomeLoca design boards): the overview a person opens
// from the top of the sidebar. Their locas, with who is there, what is new
// and the last line said; what is unread and who named them; the newest
// announcement; and the tasks given to them in the loca they came from.
//
// Home is not a room. It opens no socket and has no composer: the loca that
// was open stays connected behind it (its unread count goes on counting, as
// on any other tab), and leaving Home goes back to that loca, on the tab it
// was on. Sign-in never lands here (api.js landingRoom): the person opens
// Home, the page does not.
//
// Everything on it is what the page already holds, and Home asks the server
// for nothing:
//   - the loca cards: GET /rooms summaries (state.rooms, state.js
//     refreshRooms), the unread counts and mentions counted from the unread
//     tails (state.js updateUnreadCounts, socket.js for the open loca);
//   - private messages: state.dms with the same unread counts;
//   - notifications: the bell's poll (notifications.js, state.notif);
//   - tasks: the open loca's tasks (focus.js fetchTasks), which is the only
//     loca whose tasks the page reads. Other locas' tasks would take one
//     request per loca, so Home does not list them.

let homeReturnTab = "chat";

function homeOpen() { return state.tab === "home"; }

function openHome() {
  if (homeOpen()) return;
  homeReturnTab = state.tab || "chat";
  setMobileSidebar(false);
  switchTab("home");
}

// Back to the loca that was open, on the tab it was on; a tab its loca has
// switched off since is no longer there, and Chat is.
function leaveHome() {
  if (!homeOpen()) return;
  let back = homeReturnTab && homeReturnTab !== "home" ? homeReturnTab : "chat";
  const tab = document.querySelector(`.tabs [data-tab="${back}"]`);
  if (back !== "people" && (!tab || tab.classList.contains("hidden"))) back = "chat";
  switchTab(back);
}

// Whatever Home shows changed: draw it again, if it is open.
function refreshHome() {
  if (homeOpen()) renderHome();
}

// Home is drawn again on every room refresh and notification poll. A part
// whose content is the same is left as it is, so a card under the keyboard's
// focus keeps it.
const homeDrawn = new WeakMap();
function setHomeHtml(el, html) {
  if (homeDrawn.get(el) === html) return;
  homeDrawn.set(el, html);
  el.innerHTML = html;
}

// Signed out: what Home showed was that person's (app.js clearBuildingData).
function clearHome() {
  for (const id of ["homeLocas", "homeUnreadList", "homeNotifBody", "homeTaskList"]) {
    const el = $(id);
    if (!el) continue;
    homeDrawn.delete(el);
    el.innerHTML = "";
  }
  $("homeSub").textContent = "";
}

function homeGreeting(now = new Date()) {
  const hour = now.getHours();
  const part = hour >= 5 && hour < 12 ? "morning" : hour >= 12 && hour < 18 ? "afternoon" : "evening";
  return `Good ${part}, ${state.name || "there"}`;
}

function homePlural(n, one, many) { return `${n} ${n === 1 ? one : many}`; }

// A loca's unread count, and of those how many name this person. A mention
// count is only as fresh as the unread count it was taken with.
function homeUnread(room) { return Number(state.unread[room] || 0); }
function homeMentions(room) {
  return homeUnread(room) ? Number(state.unreadMentions?.[room] || 0) : 0;
}

function homePresence(rm) {
  const people = Number(rm.humans || 0);
  const agents = Number(rm.agents || 0);
  if (!people && !agents) return "nobody at the table";
  return [people ? homePlural(people, "person", "people") : "",
    agents ? homePlural(agents, "agent", "agents") : ""].filter(Boolean).join(" · ") + " here";
}

// The loca that was open first ("Continue"), then those with something new,
// most first, then the rest in the sidebar's own order.
function homeRooms() {
  const listed = orderedSidebarRooms(state.rooms || []);
  const rank = new Map(listed.map((rm, i) => [rm.room, i]));
  return [...listed].sort((a, b) =>
    Number(b.room === state.room) - Number(a.room === state.room)
    || homeUnread(b.room) - homeUnread(a.room)
    || rank.get(a.room) - rank.get(b.room));
}

function homeCardHtml(rm) {
  const unread = homeUnread(rm.room);
  const mentions = homeMentions(rm.room);
  const current = rm.room === state.room;
  const kicker = current ? "Continue"
    : isRankHome(rm.room) ? "Care"
    : rm.group ? rm.group : "Ground floor";
  const mark = locaMarkHtml(rm) || `<span class="locamark" aria-hidden="true">#</span>`;
  const news = unread
    ? `<span class="homenew">${unread > 99 ? "99+" : unread} new</span>` +
      (mentions ? `<span class="homementioned">${mentions === 1 ? "@ you" : `@ you ×${mentions}`}</span>` : "")
    : "";
  const closed = rm.archived ? `<span class="homeclosed">closed</span>` : "";
  return `<button type="button" class="homecard${unread ? " has-unread" : ""}${current ? " current" : ""}" data-home-room="${esc(rm.room)}" title="${esc(locaRowTitle(rm))}">` +
    `<span class="homekicker">${esc(kicker)}</span>` +
    `<span class="homename">${mark}<span class="homenametext">${esc(locaName(rm.room))}</span>${closed}</span>` +
    `<span class="homewho">${esc(homePresence(rm))}</span>` +
    `<span class="homelast">${rm.last ? esc(rm.last) : `<span class="homequiet">nothing said yet</span>`}</span>` +
    (news ? `<span class="homenews">${news}</span>` : "") +
    `</button>`;
}

function renderHomeLocas() {
  const rooms = homeRooms();
  setHomeHtml($("homeLocas"), rooms.length
    ? rooms.map(homeCardHtml).join("")
    : `<p class="homeempty">No loca has a seat for you yet. When someone calls you in, it appears here and in the sidebar.</p>`);
}

// Unread, by where it is: locas that name this person first, then the rest,
// the Lobby room, then private messages.
function renderHomeUnread() {
  const rows = [];
  const locas = (state.rooms || []).filter(rm => homeUnread(rm.room))
    .sort((a, b) => homeMentions(b.room) - homeMentions(a.room) || homeUnread(b.room) - homeUnread(a.room));
  for (const rm of locas) {
    const mentions = homeMentions(rm.room);
    rows.push({ room: rm.room, name: locaName(rm.room), count: homeUnread(rm.room),
      note: mentions ? (mentions === 1 ? "names you" : `names you ${mentions} times`) : "" });
  }
  if (state.lobbyRoom && homeUnread(LOBBY_ROOM)) {
    rows.push({ room: LOBBY_ROOM, name: "Lobby", count: homeUnread(LOBBY_ROOM), note: "" });
  }
  for (const dm of state.dms || []) {
    if (homeUnread(dm.room)) rows.push({ room: dm.room, name: dm.peer, count: homeUnread(dm.room), note: "private", dm: true });
  }
  const total = rows.reduce((n, r) => n + r.count, 0);
  const named = rows.reduce((n, r) => n + homeMentions(r.room), 0);
  $("homeUnreadTitle").textContent = total ? `Unread · ${total > 999 ? "999+" : total}` : "Unread";
  setHomeHtml($("homeUnreadList"), rows.length
    ? (named ? `<p class="homelead">${homePlural(named, "message names", "messages name")} you.</p>` : "") +
      `<ul class="homelist">${rows.map(r =>
        `<li><button type="button" class="homerow${homeMentions(r.room) ? " mentioned" : ""}" data-home-room="${esc(r.room)}">` +
          `<span class="homerowname">${r.dm ? "" : r.room === LOBBY_ROOM ? "" : "# "}${esc(r.name)}</span>` +
          (r.note ? `<span class="homerownote">${esc(r.note)}</span>` : "") +
          `<span class="homecount">${r.count > 99 ? "99+" : r.count}</span>` +
        `</button></li>`).join("")}</ul>`
    : `<p class="homeempty">Nothing unread. You are caught up.</p>`);
  return { total, rooms: rows.length };
}

// The newest announcement, unread first, as the bell has it.
function renderHomeNotifs() {
  const n = state.notif;
  const shown = !!n && notifSignedIn() && n.loaded && !n.denied;
  $("homeNotifs").classList.toggle("hidden", !shown);
  if (!shown) return;
  const items = n.items || [];
  const latest = items.find(x => !x.read) || items[0];
  $("homeNotifsTitle").textContent = n.unread ? `Notifications · ${n.unread} unread` : "Notifications";
  setHomeHtml($("homeNotifBody"), (latest
    ? `<article class="homenotif${latest.read ? "" : " unread"}">` +
        `<span class="homekicker">From ${esc(latest.from || "the Building")} · <time datetime="${new Date(latest.created_at).toISOString()}" title="${esc(fmtFull(latest.created_at))}">${esc(notifWhen(latest.created_at))}</time></span>` +
        `<b class="homenotiftitle">${esc(latest.title)}</b>` +
        (latest.body ? `<p class="homenotifbody">${esc(latest.body)}</p>` : "") +
      `</article>`
    : `<p class="homeempty">No announcements yet.</p>`) +
    `<button type="button" class="homemore" data-home-notifs>All notifications</button>`);
}

// The tasks given to this person in the loca that was open: the only tasks
// the page has read. Shown once that loca's list is in, and only where the
// Loca Board is on.
function renderHomeTasks() {
  const room = state.room;
  const shown = !!room && !isDm(room) && !isLobbyRoom(room)
    && typeof extensionOn === "function" && extensionOn("board") && isLocaContextReady("goals");
  $("homeTasks").classList.toggle("hidden", !shown);
  if (!shown) return;
  const mine = t => t.assigned_principal && state.principalId
    ? t.assigned_principal === state.principalId
    : t.assigned_to === state.name;
  const order = { taken: 0, review: 1, open: 2 };
  const held = Object.values(state.tasks || {})
    .filter(t => t.room === room && order[t.status] !== undefined && t.assigned_to && mine(t))
    .sort((a, b) => order[a.status] - order[b.status] || a.id - b.id);
  $("homeTasksTitle").textContent = `Your tasks in ${roomLabel(room)}`;
  setHomeHtml($("homeTaskList"), held.length
    ? `<ul class="homelist">${held.map(t =>
        `<li><button type="button" class="homerow" data-task-ref="${Number(t.id)}">` +
          `<span class="homerowname">#${Number(t.id)} ${esc(t.title)}</span>` +
          `<span class="homerownote">${esc(TLABEL[t.status] || t.status)}</span>` +
        `</button></li>`).join("")}</ul>`
    : `<p class="homeempty">Nothing is given to you here.</p>`);
}

function renderHome() {
  if (!$("homePanel")) return;
  $("homeGreeting").textContent = homeGreeting();
  renderHomeLocas();
  const { total, rooms } = renderHomeUnread();
  $("homeSub").textContent = total
    ? `${homePlural(total, "new message", "new messages")} in ${homePlural(rooms, "place", "places")} since you last looked.`
    : "Nothing new since you last looked.";
  renderHomeNotifs();
  renderHomeTasks();
}

$("homeLink").addEventListener("click", () => {
  if (homeOpen()) { setMobileSidebar(false); leaveHome(); } else openHome();
});

$("homePanel").addEventListener("click", (e) => {
  if (e.target.closest("[data-home-notifs]")) { openNotifPanel(true); return; }
  const go = e.target.closest("[data-home-room]");
  if (!go) return;
  const room = go.dataset.homeRoom;
  // The loca that was open is still connected: go back to it as it was.
  if (room === state.room) { leaveHome(); return; }
  joinRoom(room);   // a Building view gives way to the loca's chat (socket.js)
});
