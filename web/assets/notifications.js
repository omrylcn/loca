"use strict";
// Notifications (docs/concepts.md "Notifications"): Building announcements
// from the Master to everyone, a floor, the ranks or every agent; from a
// Smaster to its own groups or the ranks; from the caretaker to every agent. A bell in
// the sidebar header carries the unread count; its panel lists them, marks
// them read and — for a sender — offers a New notification form.
//
// Delivery: the page already polls /health every 5 s (api.js). That tick also
// asks `GET /notifications?since=<newest id>`, so a new one shows within one
// tick whatever loca (or none) the person has open; before that poll starts
// (a restored session with no loca), a 5 s tick of our own stands in. The
// server decides who
// may send to whom; this file only stops offering what would be refused.

state.notif = {
  principal: null, loaded: false, items: [], unread: 0, more: false,
  newest: 0, catchup: null, inflight: false, open: false, view: "inbox",
  audiences: null, sent: null, toastTimer: 0, full: false,
};

const NOTIF_TITLE_MAX = 120;
const NOTIF_BODY_MAX = 2000;
const NOTIF_X_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';
// An announcement's mark in the list: a megaphone on a small tile.
const NOTIF_ANNOUNCE_ICON = '<span class="notificon" aria-hidden="true"><svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3 11l14-6v14L3 13z"/><path d="M7 13v5a2 2 0 0 0 4 0v-4"/></svg></span>';
const NOTIF_BELL_SMALL = '<svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>';

/// Who the panel belongs to: the credential this page signs requests with.
/// `state.principalId` is only known once a loca's profile loads, and a
/// person with no loca has none, so the credential is the key instead.
function notifIdentity() {
  return state.session || state.roomToken || "";
}

function notifSignedIn() {
  return !document.body.classList.contains("locked") && !!notifIdentity();
}

function notifReset() {
  const n = state.notif;
  Object.assign(n, {
    principal: notifIdentity(), loaded: false, denied: false, items: [], unread: 0, more: false,
    newest: 0, catchup: null, audiences: null, sent: null, view: "inbox",
  });
  closeNotifPanel();
}

/// Called on every health tick (api.js) and when the bell opens.
async function pollNotifications() {
  const n = state.notif;
  if (!notifSignedIn()) { renderNotifBell(); return; }
  if (n.principal !== notifIdentity()) notifReset();
  if (n.inflight) return;
  n.inflight = true;
  const principal = n.principal;
  // The credential alone misses a sign-out and back in on the same one; the
  // Building generation does not.
  const gen = state.buildingGen;
  const stale = () => principal !== notifIdentity() || buildingStale(gen);
  try {
    // Keep the original lower watermark until the whole interval is drained.
    // One bounded page per tick; a failed request retains the exact checkpoint.
    const query = n.loaded
      ? `?since=${n.newest}${n.catchup ? `&before=${n.catchup.before}` : ""}`
      : "?limit=30";
    const session = state.session;
    const r = await fetch(`${serverBase()}/notifications${query}`, { headers: adminHeaders({}) });
    if (stale()) return;
    if (!r.ok) {
      // No person behind this credential (a bare invite, a dev door): no bell.
      // A 401 for a session replaced since says nothing about the new one.
      if (r.status === 401 && session === state.session) { n.loaded = false; n.unread = 0; n.denied = true; renderNotifBell(); }
      return;
    }
    n.denied = false;
    const page = await r.json();
    if (stale()) return;
    const list = Array.isArray(page.notifications) ? page.notifications : [];
    const known = new Set(n.items.map(x => x.id));
    const fresh = list.filter(x => !known.has(x.id));
    const initial = !n.loaded;
    if (initial) {
      n.items = list;
      n.more = !!page.more;
      n.loaded = true;
    } else if (fresh.length) {
      n.items = [...fresh, ...n.items].sort((a, b) => b.id - a.id);
      const unreadFresh = fresh.filter(x => !x.read);
      if (unreadFresh.length) showNotifToast(unreadFresh);
      soundForNotices(unreadFresh);
    }
    const high = Math.max(n.newest, n.catchup?.newest || 0, ...list.map(x => x.id));
    if (!initial && page.more && list.length) {
      n.catchup = { before: Math.min(...list.map(x => x.id)), newest: high };
    } else {
      n.newest = high;
      n.catchup = null;
    }
    n.unread = Number(page.unread) || 0;
    renderNotifBell();
    if (n.open) renderNotifPanel();
  } catch (e) { /* the health line owns transport errors */ }
  finally { if (!buildingStale(gen)) n.inflight = false; }   // a stale poll no longer holds the flag
}

function renderNotifBell() {
  const bell = $("notifBell");
  if (!bell) return;
  const on = notifSignedIn() && state.notif.loaded && !state.notif.denied;
  bell.classList.toggle("hidden", !on);
  const unread = on ? state.notif.unread : 0;
  const badge = $("notifBadge");
  badge.textContent = unread > 99 ? "99+" : String(unread);
  badge.classList.toggle("hidden", unread === 0);
  bell.setAttribute("aria-label", unread ? `Notifications, ${unread} unread` : "Notifications");
  bell.dataset.tipDetail = unread
    ? `${unread} unread. Announcements from the Master and your floors' Smasters.`
    : "Announcements from the Master and your floors' Smasters.";
  // The phone's closed drawer hides the bell; a dot on the menu toggle says
  // there is something to read without adding a button to the top bar.
  document.body.classList.toggle("notif-unread", unread > 0);
  if (window.refreshTip) window.refreshTip(bell);
  // Home carries the same count and the newest announcement (home.js).
  if (typeof refreshHome === "function") refreshHome();
}

function notifWhen(ts) {
  if (!ts) return "";
  const d = new Date(ts);
  const today = new Date();
  if (d.toDateString() === today.toDateString()) return fmtTime(ts);
  return d.toLocaleDateString([], { month: "short", day: "numeric" }) + " " + fmtTime(ts);
}

function notifAudienceLabel(a) {
  if (!a) return "";
  if (a.all) return "everyone";
  if (a.agents) return "every agent";
  if (a.ranks) return "the Master and the Smasters";
  const groups = (a.groups || []).map(g => typeof g === "string" ? g : g.name).filter(Boolean);
  return groups.length ? groups.join(", ") : "your floor";
}

function ensureNotifPanel() {
  let panel = $("notifPanel");
  if (panel) return panel;
  panel = document.createElement("section");
  panel.id = "notifPanel";
  panel.className = "notifpanel hidden";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Notifications");
  panel.addEventListener("click", (e) => onNotifPanelClick(e));
  panel.addEventListener("submit", (e) => onNotifSubmit(e));
  panel.addEventListener("change", (e) => onNotifChange(e));
  panel.addEventListener("input", (e) => onNotifInput(e));
  document.body.appendChild(panel);
  return panel;
}

async function openNotifPanel(full = false) {
  const n = state.notif;
  const panel = ensureNotifPanel();
  n.full = !!full;
  panel.classList.toggle("notiffull", n.full);
  // On a phone the panel is a sheet of its own: put the drawer away first.
  if (window.matchMedia("(max-width: 760px)").matches) setMobileSidebar(false);
  n.open = true;
  panel.classList.remove("hidden");
  $("notifBell").setAttribute("aria-expanded", "true");
  placeNotifPanel();
  renderNotifPanel();
  hideNotifToast();
  const gen = state.buildingGen;
  await Promise.all([pollNotifications(), fetchNotifAudiences()]);
  if (buildingStale(gen)) return;
  renderNotifPanel();
  panel.querySelector(".notifhead h2")?.focus?.();
}

function closeNotifPanel() {
  const n = state.notif;
  n.open = false;
  n.full = false;
  $("notifPanel")?.classList.add("hidden");
  $("notifPanel")?.classList.remove("notiffull");
  $("notifBell")?.setAttribute("aria-expanded", "false");
}

function toggleNotifPanel() {
  if (state.notif.open) closeNotifPanel(); else openNotifPanel();
}

/// "Open all": the same list, laid over the main area as a page of its own.
function openNotifFull() {
  const panel = ensureNotifPanel();
  state.notif.full = true;
  panel.classList.add("notiffull");
  placeNotifPanel();
  renderNotifPanel();
  panel.querySelector(".notifhead h2")?.focus?.();
}

/// Beside the bell on a wide screen; a full-width sheet on a phone (CSS).
/// The full view ("Open all") lies over the main area instead.
function placeNotifPanel() {
  const panel = $("notifPanel");
  const bell = $("notifBell");
  if (!panel || !bell) return;
  if (state.notif.full) {
    const main = document.querySelector("main.main");
    const m = main ? main.getBoundingClientRect() : { left: 0, top: 0, width: window.innerWidth, height: window.innerHeight };
    panel.style.setProperty("--notif-left", `${Math.round(m.left)}px`);
    panel.style.setProperty("--notif-top", `${Math.round(m.top)}px`);
    panel.style.setProperty("--notif-width", `${Math.round(m.width)}px`);
    panel.style.setProperty("--notif-height", `${Math.round(m.height)}px`);
    return;
  }
  const r = bell.getBoundingClientRect();
  const width = Math.min(460, window.innerWidth - 16);   // page-people.css #notifPanel
  const left = Math.max(8, Math.min(r.left - 24, window.innerWidth - width - 8));
  panel.style.setProperty("--notif-left", `${Math.round(left)}px`);
  panel.style.setProperty("--notif-top", `${Math.round(r.bottom + 6)}px`);
}

async function fetchNotifAudiences() {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/notifications/audiences`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    const audiences = r.ok ? await r.json() : null;
    if (buildingStale(gen)) return;
    state.notif.audiences = audiences;
  } catch (e) { if (!buildingStale(gen)) state.notif.audiences = null; }
}

async function fetchNotifSent() {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/notifications/sent?limit=30`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    const sent = r.ok ? await r.json() : [];
    if (buildingStale(gen)) return;
    state.notif.sent = sent;
  } catch (e) { if (!buildingStale(gen)) state.notif.sent = []; }
}

function notifItemHtml(x) {
  const unread = !x.read;
  return `<li class="notifitem${unread ? " unread" : ""}" data-notif="${x.id}">` + NOTIF_ANNOUNCE_ICON +
    `<div class="notifmain"><div class="notifitemhead"><b class="notiftitle">${esc(x.title)}</b>` +
      (unread ? `<button type="button" class="btn notifmark" data-notif-read="${x.id}" aria-label="Mark ${esc(x.title)} read">Mark read</button>` : "") +
    `</div>` +
    (x.body ? `<p class="notifbody">${esc(x.body)}</p>` : "") +
    `<div class="notifmeta"><span class="notiffrom">${esc(x.from)}</span>` +
      `<span class="notiffrom">to ${esc(notifAudienceLabel(x.audience))}</span><span aria-hidden="true">·</span>` +
      `<time datetime="${new Date(x.created_at).toISOString()}" title="${esc(fmtFull(x.created_at))}">${esc(notifWhen(x.created_at))}</time></div>` +
    `</div></li>`;
}

// Who a sent notification went to, as the sender reads it. People and agents
// are told apart: for an agent a notification is context, not something it
// reads, so "read" is counted among people only. A server from before that
// split sends neither count; everyone is then counted as people, as it did.
function notifReach(x) {
  const agents = Number(x.agents || 0);
  const people = x.people === undefined ? Number(x.recipients || 0) - agents : Number(x.people || 0);
  return { people, agents };
}
function notifCountWord(n, one, many) { return `${n} ${n === 1 ? one : many}`; }
function notifSentToText(x) {
  const { people, agents } = notifReach(x);
  const parts = [];
  if (people || !agents) parts.push(notifCountWord(people, "person", "people"));
  if (agents) parts.push(notifCountWord(agents, "agent", "agents"));
  return `Sent to ${parts.join(" and ")}.`;
}
function notifCountsText(x) {
  const { people, agents } = notifReach(x);
  const parts = [];
  if (people || !agents) parts.push(`${Number(x.read || 0)} of ${people} read`);
  if (agents) parts.push(`sent to ${notifCountWord(agents, "agent", "agents")}`);
  // Reached: the recipient's own client fetched it. Not a read.
  if (x.delivered !== undefined) parts.push(`reached ${Number(x.delivered || 0)} of ${Number(x.recipients || 0)}`);
  return parts.join(" · ");
}

// Who a sent notification went to and whom it reached, fetched when asked.
const notifRecipients = new Map();
function notifRecipientsHtml(id) {
  const list = notifRecipients.get(id);
  if (!list) return "";
  if (list === "loading") return `<div class="notifwho">Loading…</div>`;
  if (list === "error") return `<div class="notifwho">Could not load who it reached.</div>`;
  return `<ul class="notifwho">` + list.map(r =>
    `<li><span>${esc(r.name)}</span><small>${r.kind === "agent" ? "agent" : "person"}</small>` +
    (r.delivered_at
      ? `<span class="notifreached" title="${esc(fmtFull(r.delivered_at))}">reached ${esc(notifWhen(r.delivered_at))}</span>`
      : `<span class="notifwaiting">not yet</span>`) +
    (r.read_at ? `<span>read</span>` : "") + `</li>`).join("") + `</ul>`;
}
async function toggleNotifRecipients(id) {
  if (notifRecipients.has(id)) { notifRecipients.delete(id); renderNotifPanel(); return; }
  notifRecipients.set(id, "loading");
  renderNotifPanel();
  try {
    const r = await fetch(`${serverBase()}/notifications/${encodeURIComponent(id)}/recipients`, { headers: adminHeaders({}) });
    notifRecipients.set(id, r.ok ? (await r.json()).recipients || [] : "error");
  } catch (e) { notifRecipients.set(id, "error"); }
  renderNotifPanel();
}

function notifSentHtml(x) {
  return `<li class="notifitem sent" data-notif-sent="${x.id}">` + NOTIF_ANNOUNCE_ICON +
    `<div class="notifmain"><div class="notifitemhead"><b class="notiftitle">${esc(x.title)}</b></div>` +
    (x.body ? `<p class="notifbody">${esc(x.body)}</p>` : "") +
    `<div class="notifmeta">${x.from ? `<span class="notiffrom">from ${esc(x.from)}</span><span aria-hidden="true">·</span>` : ""}` +
      `<span class="notiffrom">to ${esc(notifAudienceLabel(x.audience))}</span><span aria-hidden="true">·</span>` +
      `<time datetime="${new Date(x.created_at).toISOString()}" title="${esc(fmtFull(x.created_at))}">${esc(notifWhen(x.created_at))}</time></div>` +
    `<div class="notifcounts">${esc(notifCountsText(x))} ` +
      `<button type="button" class="linkbtn" data-notif-who="${x.id}" aria-expanded="${notifRecipients.has(x.id)}">${notifRecipients.has(x.id) ? "hide who" : "who it reached"}</button></div>` +
    notifRecipientsHtml(x.id) +
    `</div></li>`;
}

function notifComposerHtml(a) {
  // Whole audiences, one at a time: everyone (the Master), every agent (the
  // Master, the caretaker), the ranks (every rank). Groups are the other way.
  const wholes = [];
  if (a.everyone) wholes.push(["all", "Everyone in the Building"]);
  if (a.agents) wholes.push(["agents", "Every agent"]);
  if (a.ranks) wholes.push(["ranks", "The Master and the Smasters"]);
  const groupsHere = (a.groups || []).length > 0;
  // Ticked to start with: everyone for the Master; the only choice there is.
  const preset = a.everyone ? "all" : (!groupsHere && wholes.length === 1 ? wholes[0][0] : "");
  const everyone = wholes.map(([value, label]) =>
    `<label class="notifaud"><input type="checkbox" name="whole" value="${value}"${value === preset ? " checked" : ""}> <span>${label}</span></label>`).join("");
  const groups = (a.groups || []).map(g =>
    `<label class="notifaud"><input type="checkbox" name="group" value="${esc(g.id)}"${preset ? " disabled" : ""}> ` +
    `<span>${esc(g.name)}</span><small>${g.members} ${g.members === 1 ? "person" : "people"}</small></label>`).join("");
  return `<details class="notifcompose"${state.notif.composeOpen ? " open" : ""}>` +
    `<summary>New notification</summary>` +
    `<form id="notifForm" novalidate>` +
      `<fieldset class="notifauds"><legend>To</legend>${everyone}${groups}</fieldset>` +
      `<label class="notiffield"><span>Title</span><input id="notifTitle" name="title" maxlength="${NOTIF_TITLE_MAX}" autocomplete="off" required></label>` +
      `<label class="notiffield"><span>Message <small id="notifCount">0/${NOTIF_BODY_MAX}</small></span>` +
        `<textarea id="notifText" name="body" rows="3" maxlength="${NOTIF_BODY_MAX}"></textarea></label>` +
      `<div class="notiferr" id="notifErr" role="alert"></div>` +
      `<div class="notifformacts"><button type="submit" class="btn primary" id="notifSend">Send</button></div>` +
    `</form></details>`;
}

function renderNotifPanel() {
  const n = state.notif;
  const panel = $("notifPanel");
  if (!panel || !n.open) return;
  // Keep a half-written notification across re-renders (the poll ticks).
  const draft = panel.querySelector("#notifForm") ? {
    title: $("notifTitle").value, body: $("notifText").value,
    whole: panel.querySelector("input[name=whole]:checked")?.value || "",
    groups: [...panel.querySelectorAll("input[name=group]:checked")].map(i => i.value),
    err: $("notifErr").textContent,
  } : null;
  const a = n.audiences;
  const canSend = !!(a && a.can_send);
  // The views that exist: Inbox, and Sent for a sender. The full view always
  // shows its chip row (the board's filters), Inbox alone for a reader.
  const tabs = canSend || n.full
    ? `<div class="notiftabs" role="group" aria-label="Notification views">` +
        `<button type="button" class="chip${n.view === "inbox" || !canSend ? " on" : ""}" data-notif-view="inbox" aria-pressed="${n.view === "inbox" || !canSend}">Inbox</button>` +
        (canSend ? `<button type="button" class="chip${n.view === "sent" ? " on" : ""}" data-notif-view="sent" aria-pressed="${n.view === "sent"}">Sent</button>` : "") +
      `</div>`
    : "";
  // The full view sorts the list under Today and Earlier, as the board does.
  const listHtml = (items, itemHtml) => {
    if (!n.full) return `<ul class="notiflist">${items.map(itemHtml).join("")}</ul>`;
    const today = new Date().toDateString();
    const parts = [["Today", items.filter(x => new Date(x.created_at).toDateString() === today)],
      ["Earlier", items.filter(x => new Date(x.created_at).toDateString() !== today)]];
    return parts.filter(([, some]) => some.length).map(([title, some]) =>
      `<h3 class="notifday">${title}</h3><ul class="notiflist">${some.map(itemHtml).join("")}</ul>`).join("");
  };
  let list;
  if (n.view === "sent" && canSend) {
    const sent = n.sent || [];
    list = n.sent === null ? `<div class="notifempty">Loading…</div>`
      : sent.length ? listHtml(sent, notifSentHtml)
      : `<div class="notifempty">You have not sent a notification yet.</div>`;
  } else {
    list = !n.loaded ? `<div class="notifempty">Loading…</div>`
      : n.items.length ? listHtml(n.items, notifItemHtml) +
          (n.more ? `<button type="button" class="btn notifolder" data-notif-older>Show older</button>` : "")
      : `<div class="notifempty">No notifications yet.</div>`;
  }
  panel.innerHTML =
    `<div class="notifhead"><h2 tabindex="-1">${NOTIF_BELL_SMALL}<span>Notifications</span>` +
      (n.unread ? `<span class="notifcount">${n.unread} unread</span>` : "") + `</h2>` +
      (n.full ? "" : `<button type="button" class="btn notifopenall" data-notif-full>Open all</button>`) +
      `<button type="button" class="btn" data-notif-readall${n.unread ? "" : " disabled"}>Mark all read</button>` +
      `<button type="button" class="iconbtn notifclose" data-notif-close aria-label="Close notifications">${NOTIF_X_ICON}</button></div>` +
    (canSend ? notifComposerHtml(a) : "") + tabs +
    `<div class="notifscroll">${list}</div>`;
  if (draft) {
    $("notifTitle").value = draft.title;
    $("notifText").value = draft.body;
    for (const box of panel.querySelectorAll("input[name=whole]")) box.checked = box.value === draft.whole;
    for (const box of panel.querySelectorAll("input[name=group]")) {
      box.checked = draft.groups.includes(box.value);
      box.disabled = !!draft.whole;
    }
    $("notifErr").textContent = draft.err;
    updateNotifCount();
  }
}

function updateNotifCount() {
  const count = $("notifCount");
  if (count) count.textContent = `${$("notifText").value.length}/${NOTIF_BODY_MAX}`;
}

async function markNotifRead(id) {
  const gen = state.buildingGen;
  const r = await fetch(`${serverBase()}/notifications/${encodeURIComponent(id)}/read`, {
    method: "POST", headers: adminHeaders({}),
  }).catch(() => null);
  if (!r || !r.ok || buildingStale(gen)) return;
  const item = state.notif.items.find(x => x.id === id);
  if (item && !item.read) {
    item.read = true;
    state.notif.unread = Math.max(0, state.notif.unread - 1);
  }
  renderNotifBell();
  renderNotifPanel();
}

async function markAllNotifRead() {
  const gen = state.buildingGen;
  const r = await fetch(`${serverBase()}/notifications/read-all`, {
    method: "POST", headers: adminHeaders({}),
  }).catch(() => null);
  if (!r || !r.ok || buildingStale(gen)) return;
  for (const x of state.notif.items) x.read = true;
  state.notif.unread = 0;
  renderNotifBell();
  renderNotifPanel();
}

async function loadOlderNotifs() {
  const n = state.notif;
  const oldest = n.items.length ? n.items[n.items.length - 1].id : null;
  if (oldest === null) return;
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/notifications?before=${oldest}&limit=30`, { headers: adminHeaders({}) });
    if (!r.ok || buildingStale(gen)) return;
    const page = await r.json();
    if (buildingStale(gen)) return;
    const known = new Set(n.items.map(x => x.id));
    n.items = [...n.items, ...(page.notifications || []).filter(x => !known.has(x.id))];
    n.more = !!page.more;
  } catch (e) { return; }
  renderNotifPanel();
}

async function sendNotif() {
  const panel = $("notifPanel");
  const err = $("notifErr");
  const title = $("notifTitle").value.trim();
  const body = $("notifText").value;
  const whole = panel.querySelector("input[name=whole]:checked")?.value || "";
  const groups = [...panel.querySelectorAll("input[name=group]:checked")].map(i => i.value);
  err.textContent = "";
  if (!title) { err.textContent = "Give it a title."; $("notifTitle").focus(); return; }
  if (!whole && !groups.length) { err.textContent = "Choose who receives it."; return; }
  const button = $("notifSend");
  button.disabled = true;
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/notifications`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title, body, audience: whole ? { [whole]: true } : { groups } }),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const text = await r.text();
      if (!buildingStale(gen)) err.textContent = text || `could not send (${r.status})`;
      return;
    }
    const sent = await r.json();
    if (buildingStale(gen)) return;
    $("notifTitle").value = "";
    $("notifText").value = "";
    updateNotifCount();
    state.notif.sent = null;
    state.notif.composeOpen = false;
    state.notif.view = "sent";
    await fetchNotifSent();
    if (buildingStale(gen)) return;
    renderNotifPanel();
    const head = $("notifPanel").querySelector(".notifhead h2");
    if (head) head.focus();
    showNotifStatus(notifSentToText(sent));
  } catch (e) {
    if (!buildingStale(gen)) err.textContent = "could not reach the server — try again";
  } finally {
    if ($("notifSend")) $("notifSend").disabled = false;
  }
}

function showNotifStatus(text) {
  const panel = $("notifPanel");
  if (!panel) return;
  let line = panel.querySelector(".notifstatus");
  if (!line) {
    line = document.createElement("div");
    line.className = "notifstatus";
    line.setAttribute("role", "status");
    panel.querySelector(".notifhead")?.after(line);
  }
  line.textContent = text;
}

function onNotifPanelClick(e) {
  const t = e.target.closest("button, summary");
  if (!t) return;
  if (t.matches("summary")) { state.notif.composeOpen = !t.parentElement.open; return; }
  if (t.dataset.notifClose !== undefined) { closeNotifPanel(); $("notifBell").focus(); return; }
  if (t.dataset.notifFull !== undefined) { openNotifFull(); return; }
  if (t.dataset.notifReadall !== undefined) { markAllNotifRead(); return; }
  if (t.dataset.notifRead) { markNotifRead(Number(t.dataset.notifRead)); return; }
  if (t.dataset.notifOlder !== undefined) { loadOlderNotifs(); return; }
  if (t.dataset.notifWho) { toggleNotifRecipients(Number(t.dataset.notifWho)); return; }
  if (t.dataset.notifView) {
    state.notif.view = t.dataset.notifView;
    if (state.notif.view === "sent") {
      state.notif.sent = null;
      renderNotifPanel();
      fetchNotifSent().then(() => renderNotifPanel());
    } else {
      renderNotifPanel();
    }
  }
}

function onNotifSubmit(e) {
  if (e.target.id !== "notifForm") return;
  e.preventDefault();
  sendNotif();
}

function onNotifChange(e) {
  if (e.target.name !== "whole") return;
  const panel = $("notifPanel");
  if (e.target.checked) {
    for (const other of panel.querySelectorAll("input[name=whole]")) if (other !== e.target) other.checked = false;
  }
  const whole = !!panel.querySelector("input[name=whole]:checked");
  for (const box of panel.querySelectorAll("input[name=group]")) {
    box.disabled = whole;
    if (whole) box.checked = false;
  }
}

function onNotifInput(e) {
  if (e.target.id === "notifText") updateNotifCount();
  if (e.target.id === "notifTitle" || e.target.id === "notifText") $("notifErr").textContent = "";
}

/* ---- toast: one at a time, dismissible, gone after a few seconds ---- */
function showNotifToast(fresh) {
  let toast = $("notifToast");
  if (!toast) {
    toast = document.createElement("div");
    toast.id = "notifToast";
    toast.className = "notiftoast hidden";
    toast.setAttribute("role", "status");
    toast.setAttribute("aria-live", "polite");
    toast.addEventListener("click", (e) => onNotifToastClick(e));
    document.body.appendChild(toast);
  }
  const first = fresh[0];
  const more = fresh.length > 1 ? `<small>and ${fresh.length - 1} more</small>` : "";
  toast.innerHTML =
    `<button type="button" class="notiftoastopen" data-notif-toast-open>` +
      `${NOTIF_BELL_SMALL}<span><b>${esc(first.title)}</b><small>from ${esc(first.from)}</small>${more}</span></button>` +
    `<button type="button" class="iconbtn" data-notif-toast-close aria-label="Dismiss">${NOTIF_X_ICON}</button>`;
  toast.classList.remove("hidden");
  clearTimeout(state.notif.toastTimer);
  state.notif.toastTimer = setTimeout(() => hideNotifToast(), 8000);
}

function hideNotifToast() {
  clearTimeout(state.notif.toastTimer);
  $("notifToast")?.classList.add("hidden");
}

function onNotifToastClick(e) {
  const t = e.target.closest("button");
  if (!t) return;
  hideNotifToast();
  if (t.dataset.notifToastOpen !== undefined) openNotifPanel();
}

$("notifBell").addEventListener("click", (e) => { e.stopPropagation(); toggleNotifPanel(); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.notif.open) { closeNotifPanel(); $("notifBell").focus(); }
});
document.addEventListener("pointerdown", (e) => {
  if (!state.notif.open) return;
  if (e.target.closest("#notifPanel, #notifBell, #notifToast")) return;
  closeNotifPanel();
});
window.addEventListener("resize", () => { if (state.notif.open) placeNotifPanel(); });
// A restored session with no loca open never starts the health poll (api.js
// starts it on connect). Until it runs, keep the same 5 s rhythm here; once it
// runs, it carries the notification poll and this tick stands aside.
setInterval(() => { if (!healthTimer) pollNotifications(); }, 5000);
renderNotifBell();
