"use strict";
// Notifications (docs/concepts.md "Notifications"): Building announcements
// from the Master to everyone, or from a Smaster to its own groups. A bell in
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
  newest: 0, inflight: false, open: false, view: "inbox",
  audiences: null, sent: null, toastTimer: 0,
};

const NOTIF_TITLE_MAX = 120;
const NOTIF_BODY_MAX = 2000;
const NOTIF_X_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';
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
    newest: 0, audiences: null, sent: null, view: "inbox",
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
  try {
    const query = n.loaded ? `?since=${n.newest}` : "?limit=30";
    const r = await fetch(`${serverBase()}/notifications${query}`, { headers: adminHeaders({}) });
    if (principal !== notifIdentity()) return;
    if (!r.ok) {
      // No person behind this credential (a bare davet, a dev door): no bell.
      if (r.status === 401) { n.loaded = false; n.unread = 0; n.denied = true; renderNotifBell(); }
      return;
    }
    n.denied = false;
    const page = await r.json();
    const list = Array.isArray(page.notifications) ? page.notifications : [];
    const known = new Set(n.items.map(x => x.id));
    const fresh = list.filter(x => !known.has(x.id));
    if (!n.loaded) {
      n.items = list;
      n.more = !!page.more;
      n.loaded = true;
    } else if (fresh.length) {
      n.items = [...fresh, ...n.items];
      const unreadFresh = fresh.filter(x => !x.read);
      if (unreadFresh.length) showNotifToast(unreadFresh);
    }
    for (const x of list) n.newest = Math.max(n.newest, x.id);
    n.unread = Number(page.unread) || 0;
    renderNotifBell();
    if (n.open) renderNotifPanel();
  } catch (e) { /* the health line owns transport errors */ }
  finally { n.inflight = false; }
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
    ? `${unread} unread. Announcements from the Master and your groups' Smasters.`
    : "Announcements from the Master and your groups' Smasters.";
  // The phone's closed drawer hides the bell; a dot on the menu toggle says
  // there is something to read without adding a button to the top bar.
  document.body.classList.toggle("notif-unread", unread > 0);
  if (window.refreshTip) window.refreshTip(bell);
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
  const groups = (a.groups || []).map(g => typeof g === "string" ? g : g.name).filter(Boolean);
  return groups.length ? groups.join(", ") : "your group";
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

async function openNotifPanel() {
  const n = state.notif;
  const panel = ensureNotifPanel();
  n.open = true;
  panel.classList.remove("hidden");
  $("notifBell").setAttribute("aria-expanded", "true");
  placeNotifPanel();
  renderNotifPanel();
  hideNotifToast();
  await Promise.all([pollNotifications(), fetchNotifAudiences()]);
  renderNotifPanel();
  panel.querySelector(".notifhead h2")?.focus?.();
}

function closeNotifPanel() {
  const n = state.notif;
  n.open = false;
  $("notifPanel")?.classList.add("hidden");
  $("notifBell")?.setAttribute("aria-expanded", "false");
}

function toggleNotifPanel() {
  if (state.notif.open) closeNotifPanel(); else openNotifPanel();
}

/// Beside the bell on a wide screen; a full-width sheet on a phone (CSS).
function placeNotifPanel() {
  const panel = $("notifPanel");
  const bell = $("notifBell");
  if (!panel || !bell) return;
  const r = bell.getBoundingClientRect();
  const width = Math.min(400, window.innerWidth - 16);
  const left = Math.max(8, Math.min(r.left - 24, window.innerWidth - width - 8));
  panel.style.setProperty("--notif-left", `${Math.round(left)}px`);
  panel.style.setProperty("--notif-top", `${Math.round(r.bottom + 6)}px`);
}

async function fetchNotifAudiences() {
  try {
    const r = await fetch(`${serverBase()}/notifications/audiences`, { headers: adminHeaders({}) });
    state.notif.audiences = r.ok ? await r.json() : null;
  } catch (e) { state.notif.audiences = null; }
}

async function fetchNotifSent() {
  try {
    const r = await fetch(`${serverBase()}/notifications/sent?limit=30`, { headers: adminHeaders({}) });
    state.notif.sent = r.ok ? await r.json() : [];
  } catch (e) { state.notif.sent = []; }
}

function notifItemHtml(x) {
  const unread = !x.read;
  return `<li class="notifitem${unread ? " unread" : ""}" data-notif="${x.id}">` +
    `<div class="notifitemhead"><b class="notiftitle">${esc(x.title)}</b>` +
      (unread ? `<button type="button" class="btn notifmark" data-notif-read="${x.id}" aria-label="Mark ${esc(x.title)} read">Mark read</button>` : "") +
    `</div>` +
    `<div class="notifmeta"><span>${esc(x.from)}</span><span aria-hidden="true">·</span>` +
      `<span>to ${esc(notifAudienceLabel(x.audience))}</span><span aria-hidden="true">·</span>` +
      `<time datetime="${new Date(x.created_at).toISOString()}" title="${esc(fmtFull(x.created_at))}">${esc(notifWhen(x.created_at))}</time></div>` +
    (x.body ? `<p class="notifbody">${esc(x.body)}</p>` : "") +
    `</li>`;
}

function notifSentHtml(x) {
  return `<li class="notifitem" data-notif-sent="${x.id}">` +
    `<div class="notifitemhead"><b class="notiftitle">${esc(x.title)}</b></div>` +
    `<div class="notifmeta"><span>to ${esc(notifAudienceLabel(x.audience))}</span><span aria-hidden="true">·</span>` +
      `<time datetime="${new Date(x.created_at).toISOString()}" title="${esc(fmtFull(x.created_at))}">${esc(notifWhen(x.created_at))}</time></div>` +
    (x.body ? `<p class="notifbody">${esc(x.body)}</p>` : "") +
    `<div class="notifcounts">${x.read} of ${x.recipients} read</div>` +
    `</li>`;
}

function notifComposerHtml(a) {
  const everyone = a.everyone
    ? `<label class="notifaud"><input type="checkbox" name="all" checked> <span>Everyone in the Building</span></label>`
    : "";
  const groups = (a.groups || []).map(g =>
    `<label class="notifaud"><input type="checkbox" name="group" value="${esc(g.id)}"${a.everyone ? " disabled" : ""}> ` +
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
    all: panel.querySelector("input[name=all]")?.checked,
    groups: [...panel.querySelectorAll("input[name=group]:checked")].map(i => i.value),
    err: $("notifErr").textContent,
  } : null;
  const a = n.audiences;
  const canSend = !!(a && a.can_send);
  const tabs = canSend
    ? `<div class="notiftabs" role="group" aria-label="Notification views">` +
        `<button type="button" class="chip${n.view === "inbox" ? " on" : ""}" data-notif-view="inbox" aria-pressed="${n.view === "inbox"}">Inbox</button>` +
        `<button type="button" class="chip${n.view === "sent" ? " on" : ""}" data-notif-view="sent" aria-pressed="${n.view === "sent"}">Sent</button></div>`
    : "";
  let list;
  if (n.view === "sent" && canSend) {
    const sent = n.sent || [];
    list = n.sent === null ? `<div class="notifempty">Loading…</div>`
      : sent.length ? `<ul class="notiflist">${sent.map(notifSentHtml).join("")}</ul>`
      : `<div class="notifempty">You have not sent a notification yet.</div>`;
  } else {
    list = !n.loaded ? `<div class="notifempty">Loading…</div>`
      : n.items.length ? `<ul class="notiflist">${n.items.map(notifItemHtml).join("")}</ul>` +
          (n.more ? `<button type="button" class="btn notifolder" data-notif-older>Show older</button>` : "")
      : `<div class="notifempty">No notifications yet.</div>`;
  }
  panel.innerHTML =
    `<div class="notifhead"><h2 tabindex="-1">${NOTIF_BELL_SMALL}<span>Notifications</span>` +
      (n.unread ? `<span class="notifcount">${n.unread} unread</span>` : "") + `</h2>` +
      `<button type="button" class="btn" data-notif-readall${n.unread ? "" : " disabled"}>Mark all read</button>` +
      `<button type="button" class="iconbtn notifclose" data-notif-close aria-label="Close notifications">${NOTIF_X_ICON}</button></div>` +
    (canSend ? notifComposerHtml(a) : "") + tabs +
    `<div class="notifscroll">${list}</div>`;
  if (draft) {
    $("notifTitle").value = draft.title;
    $("notifText").value = draft.body;
    const all = panel.querySelector("input[name=all]");
    if (all) all.checked = !!draft.all;
    for (const box of panel.querySelectorAll("input[name=group]")) {
      box.checked = draft.groups.includes(box.value);
      box.disabled = !!(all && all.checked);
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
  const r = await fetch(`${serverBase()}/notifications/${encodeURIComponent(id)}/read`, {
    method: "POST", headers: adminHeaders({}),
  }).catch(() => null);
  if (!r || !r.ok) return;
  const item = state.notif.items.find(x => x.id === id);
  if (item && !item.read) {
    item.read = true;
    state.notif.unread = Math.max(0, state.notif.unread - 1);
  }
  renderNotifBell();
  renderNotifPanel();
}

async function markAllNotifRead() {
  const r = await fetch(`${serverBase()}/notifications/read-all`, {
    method: "POST", headers: adminHeaders({}),
  }).catch(() => null);
  if (!r || !r.ok) return;
  for (const x of state.notif.items) x.read = true;
  state.notif.unread = 0;
  renderNotifBell();
  renderNotifPanel();
}

async function loadOlderNotifs() {
  const n = state.notif;
  const oldest = n.items.length ? n.items[n.items.length - 1].id : null;
  if (oldest === null) return;
  try {
    const r = await fetch(`${serverBase()}/notifications?before=${oldest}&limit=30`, { headers: adminHeaders({}) });
    if (!r.ok) return;
    const page = await r.json();
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
  const all = !!panel.querySelector("input[name=all]")?.checked;
  const groups = [...panel.querySelectorAll("input[name=group]:checked")].map(i => i.value);
  err.textContent = "";
  if (!title) { err.textContent = "Give it a title."; $("notifTitle").focus(); return; }
  if (!all && !groups.length) { err.textContent = "Choose who receives it."; return; }
  const button = $("notifSend");
  button.disabled = true;
  try {
    const r = await fetch(`${serverBase()}/notifications`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title, body, audience: all ? { all: true } : { groups } }),
    });
    if (!r.ok) {
      err.textContent = (await r.text()) || `could not send (${r.status})`;
      return;
    }
    const sent = await r.json();
    $("notifTitle").value = "";
    $("notifText").value = "";
    updateNotifCount();
    state.notif.sent = null;
    state.notif.composeOpen = false;
    state.notif.view = "sent";
    await fetchNotifSent();
    renderNotifPanel();
    const head = $("notifPanel").querySelector(".notifhead h2");
    if (head) head.focus();
    showNotifStatus(`Sent to ${sent.recipients} ${sent.recipients === 1 ? "person" : "people"}.`);
  } catch (e) {
    err.textContent = "could not reach the server — try again";
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
  if (t.dataset.notifReadall !== undefined) { markAllNotifRead(); return; }
  if (t.dataset.notifRead) { markNotifRead(Number(t.dataset.notifRead)); return; }
  if (t.dataset.notifOlder !== undefined) { loadOlderNotifs(); return; }
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
  if (e.target.name !== "all") return;
  for (const box of $("notifPanel").querySelectorAll("input[name=group]")) {
    box.disabled = e.target.checked;
    if (e.target.checked) box.checked = false;
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
