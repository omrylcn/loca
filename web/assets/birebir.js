"use strict";
// Birebir: a private loca for exactly two people (PRINCIPLES "Birebir").
// The two see it under Messages; nobody else sees it at all. The Master sees
// only that it exists, in the Building view, and reads it only when one side
// calls the Master in.

const BIREBIR_PREFIX = "bb-";
state.birebirs = [];
state.birebirOverview = [];

function isBirebir(room) {
  return typeof room === "string" && room.startsWith(BIREBIR_PREFIX);
}

function birebirOf(room) {
  return state.birebirs.find(b => b.room === room) || null;
}

// What the header and lists call a room: a birebir by the other person.
function roomLabel(room) {
  if (!room) return "—";
  const b = birebirOf(room);
  if (b) return b.peer;
  return isBirebir(room) ? "private" : room;
}

async function fetchBirebirs() {
  try {
    const r = await fetch(`${serverBase()}/birebirs`, { headers: adminHeaders({}) });
    state.birebirs = r.ok ? await r.json() : [];
  } catch (e) {
    state.birebirs = [];
  }
  renderBirebirList();
  if (isBirebir(state.room)) {
    $("curRoom").textContent = roomLabel(state.room);
    renderBirebirBar();
  }
}

const BIREBIR_ICON = '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>';

function renderBirebirList() {
  const list = $("dmList");
  if (!list) return;
  const prefs = loadRoomPreferences();
  const hidden = new Set(prefs.hidden);
  const rows = state.birebirs
    // Hiding is one's own preference; a new word brings it back.
    .filter(b => !hidden.has(b.room) || Number(state.unread[b.room] || 0) > 0)
    .sort((a, b) => Number(state.roomLatest[b.room] || 0) - Number(state.roomLatest[a.room] || 0));
  $("dmSection").classList.toggle("empty", !rows.length);
  list.innerHTML = rows.length ? "" : `<div class="dmhint">Press + to write to someone privately.</div>`;
  for (const b of rows) {
    const unread = Number(state.unread[b.room] || 0);
    const el = document.createElement("button");
    el.type = "button";
    el.className = "room dm" + (unread ? " has-unread" : "") + (b.room === state.room ? " active" : "");
    el.setAttribute("aria-current", b.room === state.room ? "page" : "false");
    const badge = unread
      ? `<span class="unreadbadge" title="${unread} unread">${unread > 99 ? "99+" : unread}</span>`
      : "";
    const glyph = b.peer_kind === "agent" ? "*" : ".";
    const master = b.master_in ? `<span class="dmmaster" title="The Master is reading">Master in</span>` : "";
    el.innerHTML = `<div class="rrow"><span class="rname dmname"><span class="glyph ${esc(b.peer_kind)}">${glyph}</span>${esc(b.peer)}</span>${master}${badge}</div>`;
    el.onclick = () => { setMobileSidebar(false); joinRoom(b.room); };
    const item = document.createElement("div");
    item.className = "roomitem";
    const actions = document.createElement("details");
    actions.className = "roomprefs";
    actions.innerHTML = `<summary class="roompreftrigger" aria-label="More options for ${esc(b.peer)}" title="More options">⋯</summary>`;
    const menu = document.createElement("div");
    menu.className = "roomprefmenu";
    menu.setAttribute("role", "menu");
    const del = document.createElement("button");
    del.type = "button";
    del.className = "roompref danger";
    del.setAttribute("role", "menuitem");
    del.dataset.dmDelete = b.room;
    del.dataset.dmPeer = b.peer;
    del.setAttribute("aria-label", `Close the conversation with ${b.peer}`);
    del.textContent = "Close";
    menu.append(del);
    actions.append(menu);
    item.append(el, actions);
    list.appendChild(item);
  }
}

async function openBirebir(name) {
  const r = await fetch(`${serverBase()}/birebirs`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ name }),
  });
  if (!r.ok) {
    addSys(`could not open a birebir with ${name}: ${await r.text()}`);
    return;
  }
  const { birebir } = await r.json();
  // Opening it again is a clear wish to see it: undo one's own Hide.
  if (birebir?.room && loadRoomPreferences().hidden.includes(birebir.room)) {
    updateRoomPreference("show", birebir.room);
  }
  await fetchBirebirs();
  await refreshRooms();
  if (birebir?.room) joinRoom(birebir.room);
}

// The bar above the composer in a birebir: who can read it, honestly, and
// the one way a third person ever does — the Master, called in by a side.
function renderBirebirBar() {
  const bar = $("birebirBar");
  if (!bar) return;
  const inside = isBirebir(state.room);
  document.querySelector(".main").classList.toggle("birebir", inside);
  bar.classList.toggle("hidden", !inside);
  if (!inside) return;
  if ($("target")) $("target").value = "";
  const b = birebirOf(state.room);
  if (!$("msg").disabled) $("msg").placeholder = `say it to ${b ? b.peer : "them"}…  (Enter = send · Shift+Enter = new line)`;
  if (!b) {
    bar.innerHTML = `<span><b>Private.</b> Only the two of you see this.</span>`;
    return;
  }
  const since = b.master_since ? fmtTime(b.master_since) : "";
  let status;
  let actions = "";
  if (!b.master_in) {
    status = `<b>Private.</b> Only you and ${esc(b.peer)} see this; the Master sees only that it exists. Not encrypted: whoever runs the server can read its disk${b.peer_kind === "agent" ? ", and the agent keeps what it hears" : ""}.`;
    actions = `<button type="button" class="btn" data-bb="call" title="Let the Master read from now on">Call the Master in</button>`;
  } else {
    const history = b.history_open
      ? "everything, from the start"
      : `from ${since} on${b.history_allowed_by_me ? " (you allowed earlier history; waiting for the other side)" : ""}`;
    status = `<b>The Master is reading</b> ${history}.`;
    if (!b.history_allowed_by_me) {
      actions += `<button type="button" class="btn" data-bb="history" title="Earlier history opens only when both allow it">Allow earlier history</button>`;
    }
    if (b.master_called_by_me) {
      actions += `<button type="button" class="btn" data-bb="dismiss">Send the Master out</button>`;
    }
  }
  bar.innerHTML = `<span class="bbstatus">${status}</span><span class="bbacts">${actions}</span>`;
}

async function birebirMasterAction(action) {
  const r = await fetch(`${serverBase()}/birebirs/${encodeURIComponent(state.room)}/master`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ action }),
  });
  if (!r.ok) addSys(await r.text());
  await fetchBirebirs();
}

// ---- The Master's view: existence only ----------------------------------

async function fetchBirebirOverview() {
  const box = $("birebirOverview");
  if (!box) return;
  syncBuildingTabs();
  if (state.profile?.building_role !== "master") return;
  try {
    const r = await fetch(`${serverBase()}/building/birebirs`, { headers: adminHeaders({}) });
    state.birebirOverview = r.ok ? await r.json() : [];
  } catch (e) {
    state.birebirOverview = [];
  }
  renderBirebirOverview();
}

function renderBirebirOverview() {
  const box = $("birebirOverview");
  if (!box) return;
  const rows = state.birebirOverview;
  const body = rows.length
    ? rows.map(b => {
      const last = b.last_at ? fmtFull(b.last_at) : "no messages yet";
      const state_ = b.archived ? `<span class="pflag">closed</span>` : "";
      const reading = b.master_in
        ? `<button type="button" class="btn primary" data-bbread="${esc(b.room)}" title="You were called in">Read</button>`
        : "";
      const close = b.archived
        ? `<button type="button" class="btn" data-bbclose="${esc(b.room)}" data-archived="false">Reopen</button>` +
          `<button type="button" class="btn danger" data-bbseal="${esc(b.room)}" title="Permanently; the two cannot open a new one">Seal</button>`
        : `<button type="button" class="btn" data-bbclose="${esc(b.room)}" data-archived="true">Close</button>`;
      return `<div class="bbrow"><span class="bbpair">${esc(b.between[0])} <span class="bbamp">&amp;</span> ${esc(b.between[1])}</span>` +
        `<span class="bbwhen">${esc(last)}</span>${state_}<span class="pacts">${reading}${close}</span></div>`;
    }).join("")
    : `<div class="bbempty">No private messages yet.</div>`;
  box.innerHTML = `<div class="bbhead"><b>Private messages</b> <span>${rows.length}</span>` +
    `<small>You see only that they exist, never what is said. You can read one only when one of the two calls you in.</small></div>${body}` +
    `<div class="bbwitness hidden" id="bbWitness"></div>`;
}

async function birebirOverviewClick(event) {
  const read = event.target.closest("[data-bbread]");
  const close = event.target.closest("[data-bbclose]");
  const seal = event.target.closest("[data-bbseal]");
  if (read) {
    const room = read.dataset.bbread;
    const r = await fetch(`${serverBase()}/building/birebirs/${encodeURIComponent(room)}/messages`, { headers: adminHeaders({}) });
    const panel = $("bbWitness");
    panel.classList.remove("hidden");
    if (!r.ok) { panel.textContent = await r.text(); return; }
    const data = await r.json();
    const lines = data.messages.map(m =>
      `<div class="bbline"><span class="bbtime">${esc(fmtTime(m.ts))}</span><b>${esc(m.sender)}</b> ${esc(m.text)}</div>`).join("");
    panel.innerHTML = `<div class="bbwhead">${data.history_open ? "Full history (both allowed)" : "From the moment you were called in"}` +
      `<button type="button" class="btn" data-bbwclose>Close</button></div>${lines || '<div class="bbempty">Nothing said since you were called in.</div>'}`;
    return;
  }
  if (event.target.closest("[data-bbwclose]")) { $("bbWitness").classList.add("hidden"); return; }
  if (close) {
    await fetch(`${serverBase()}/building/birebirs/${encodeURIComponent(close.dataset.bbclose)}/close`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ archived: close.dataset.archived === "true" }),
    });
    fetchBirebirOverview();
    return;
  }
  if (seal) {
    if (!confirm("Seal this conversation for good? The two cannot start a new one, and you still cannot read it.")) return;
    await fetch(`${serverBase()}/building/birebirs/${encodeURIComponent(seal.dataset.bbseal)}/seal`, {
      method: "POST",
      headers: adminHeaders({}),
    });
    fetchBirebirOverview();
  }
}

// Close removes the conversation from one's own Messages (yes/no first).
// The other side keeps theirs; a new word from them brings it back.
function deleteBirebir(room, peer) {
  if (!confirm(`Close your conversation with ${peer}?\n\nIt leaves your Messages. ${peer} keeps theirs; if they write again, it comes back.`)) return;
  updateRoomPreference("hide", room);
  if (state.room === room) joinRoom(state.homeRoom);
  renderBirebirList();
}

// Wired with late-bound calls: sidebar.js loads after this file.
$("dmList").onclick = (event) => {
  const del = event.target.closest("[data-dm-delete]");
  if (del) {
    del.closest("details")?.removeAttribute("open");
    deleteBirebir(del.dataset.dmDelete, del.dataset.dmPeer);
    return;
  }
  roomPreferenceClick(event);
};
$("birebirBar").addEventListener("click", (event) => {
  const button = event.target.closest("[data-bb]");
  if (button) birebirMasterAction(button.dataset.bb);
});
$("birebirOverview").addEventListener("click", birebirOverviewClick);

// "New message": pick a person and open (or return to) the pair's birebir,
// without going through the Building view. The server decides who is
// eligible: ranks reach anyone, members their tablemates.
let dmCandidatesCache = [];
async function openDmPicker(open) {
  const picker = $("dmPicker");
  const show = open ?? picker.classList.contains("hidden");
  picker.classList.toggle("hidden", !show);
  $("dmNew").setAttribute("aria-expanded", String(show));
  if (!show) return;
  $("dmSearch").value = "";
  $("dmCandidates").innerHTML = `<div class="dmhint">loading…</div>`;
  try {
    const r = await fetch(`${serverBase()}/birebirs/candidates`, { headers: adminHeaders({}) });
    dmCandidatesCache = r.ok ? await r.json() : [];
  } catch (e) {
    dmCandidatesCache = [];
  }
  renderDmCandidates();
  $("dmSearch").focus();
}

function renderDmCandidates() {
  const q = $("dmSearch").value.trim().toLowerCase();
  const rows = dmCandidatesCache.filter(c => !q || c.name.toLowerCase().includes(q));
  $("dmCandidates").innerHTML = rows.length
    ? rows.map(c => {
      const glyph = c.kind === "agent" ? "*" : ".";
      const badge = c.role === "member" ? "" : rankBadge(c.role);
      return `<button type="button" class="dmcand" role="option" data-dm-pick="${esc(c.name)}">` +
        `<span class="glyph ${esc(c.kind)}">${glyph}</span><span class="dmcandname">${esc(c.name)}</span>${badge}</button>`;
    }).join("")
    : `<div class="dmhint">${dmCandidatesCache.length ? "nobody by that name" : "nobody to write to yet — sit at a loca with someone first"}</div>`;
}

$("dmNew").addEventListener("click", () => openDmPicker());
$("dmSearch").addEventListener("input", () => renderDmCandidates());
$("dmSearch").addEventListener("keydown", (event) => {
  if (event.key === "Escape") { openDmPicker(false); $("dmNew").focus(); }
  if (event.key === "Enter") {
    const first = $("dmCandidates").querySelector("[data-dm-pick]");
    if (first) first.click();
  }
});
$("dmCandidates").addEventListener("click", (event) => {
  const pick = event.target.closest("[data-dm-pick]");
  if (!pick) return;
  openDmPicker(false);
  setMobileSidebar(false);
  openBirebir(pick.dataset.dmPick);
});
