"use strict";
// The Master's key desk in the Building view: one place to issue a code, and
// the ledger of every code ever issued — when, for whom, until when it can be
// used, whether it was used, and what became of it. Secrets are shown once,
// right after they are issued, and never again; the ledger keeps only a
// fingerprint to recognise the code that was handed over.

state.keyLedger = [];
state.keyFilter = "all";

const KEY_KIND_LABEL = {
  master: "Master key",
  smaster: "Smaster key",
  member: "Membership",
  davet: "Invite",
  admission: "Admission right",
};
const KEY_STATE_LABEL = {
  unused: "unused",
  active: "in use",
  used: "used",
  expired: "expired",
  superseded: "replaced",
  revoked: "revoked",
};

async function fetchKeyLedger() {
  const box = $("keyLedger");
  if (!box) return;
  syncBuildingTabs();
  if (state.profile?.building_role !== "master") return;
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/keys`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    // A ledger that could not be read is unknown, not empty: "No keys here"
    // would tell the Master nothing was ever issued. What was last read stays
    // on screen, marked as possibly out of date.
    if (!r.ok) {
      state.keyLedgerError = true;
    } else {
      const ledger = await r.json();
      if (buildingStale(gen)) return;
      state.keyLedger = ledger;
      state.keyLedgerError = false;
    }
  } catch (e) {
    if (buildingStale(gen)) return;
    state.keyLedgerError = true;
  }
  renderKeyLedger();
}

function keyWhen(ms) {
  return ms ? new Date(ms).toLocaleString([], { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" }) : "—";
}

function renderKeyLedger() {
  const box = $("keyLedger");
  if (!box) return;
  const keepIssued = box.querySelector("#keyIssued")?.outerHTML || `<div class="keyissued hidden" id="keyIssued" role="status"></div>`;
  const filters = [
    ["all", "All"], ["unused", "Unused"], ["active", "In use"], ["used", "Used"], ["closed", "Expired · revoked"],
  ];
  const rows = state.keyLedger.filter(k => {
    if (state.keyFilter === "all") return true;
    if (state.keyFilter === "closed") return ["expired", "revoked", "superseded"].includes(k.state);
    return k.state === state.keyFilter;
  });
  const revocable = k =>
    (k.kind === "master" && k.state === "unused") ||
    (k.kind === "smaster" && (k.state === "unused" || k.state === "active")) ||
    (k.kind === "davet" && k.state === "active");
  const body = rows.length ? rows.map(k => {
    // The plain @name the key was issued for, and nothing more. A ledger row
    // records a name, not a person: after a revoke the same name can belong
    // to someone else, and their full name and icon must not appear on a key
    // that was never theirs.
    const holder = k.for_name ? esc(k.for_name) : "";
    const whom = k.kind === "davet" ? `${holder} <span class="keyloca">#${esc(k.loca || "")}</span>` : (holder || "—");
    const useBy = k.use_by && (k.state === "unused") ? `<span class="keydim">use by ${esc(keyWhen(k.use_by))}</span>` : "";
    const used = k.last_used_at ? `last used ${esc(keyWhen(k.last_used_at))}` : (k.state === "unused" ? "never used" : "");
    const act = revocable(k)
      ? `<button type="button" class="btn danger" data-key-revoke="${esc(k.id)}" data-key-label="${esc(KEY_KIND_LABEL[k.kind])} ${esc(k.fingerprint)}">Revoke</button>`
      : "";
    return `<div class="keyrow">` +
      `<span class="keywho">${whom}</span>` +
      `<span class="keykind"><b>${esc(KEY_KIND_LABEL[k.kind] || k.kind)}</b><code>${esc(k.fingerprint)}</code></span>` +
      `<span class="keydates">issued ${esc(keyWhen(k.issued_at))}${k.issued_by ? ` by ${esc(k.issued_by)}` : ""}<br>${useBy}${useBy && used ? " · " : ""}<span class="keydim">${used}</span></span>` +
      `<span class="keystate state-${esc(k.state)}">${esc(KEY_STATE_LABEL[k.state] || k.state)}</span>` +
      `<span class="pacts">${act}</span></div>`;
  }).join("") : (state.keyLedgerError ? "" : `<div class="bbempty">No keys here.</div>`);
  const unread = state.keyLedgerError
    ? `<div class="bbempty keyledgererror" role="alert">The key ledger could not be loaded.${rows.length ? " What is shown may be out of date." : ""}</div>`
    : "";
  box.innerHTML =
    `<div class="bbhead"><h2 class="bbtitle">Keys</h2><span class="bbtag">Master only</span>` +
    `<small>Every key that can open this building. Keys are never shown again; you can end one.</small></div>` +
    `<div class="keygenhead"><b>Key generator</b>` +
    `<small>Generate a key, copy it once, and every key ever generated stays recorded below.</small></div>` +
    `<form class="keyform" id="keyForm" autocomplete="off">` +
      `<select id="keyKind" aria-label="Kind of key">` +
        `<option value="master">Master key — sign in as Master</option>` +
        `<option value="smaster">Smaster key — for a person</option>` +
        `<option value="admission">Admission rights — let people join</option>` +
      `</select>` +
      `<input id="keyName" maxlength="64" placeholder="name (optional — they type it at sign-in)" aria-label="Person's name, optional" class="hidden" />` +
      `<button type="submit" class="btn primary">Generate key</button>` +
    `</form>` + keepIssued +
    `<div class="keylisthead">Generated keys <span>${state.keyLedger.length}</span> <small>for whom, when, until when, and what became of each — only the first characters are kept</small></div>` +
    `<div class="keyfilters" role="group" aria-label="Filter keys">` +
      filters.map(([v, l]) => `<button type="button" class="chip${state.keyFilter === v ? " on" : ""}" data-key-filter="${v}">${l}</button>`).join("") +
    `</div>${unread}` +
    (rows.length ? `<div class="keyhead" aria-hidden="true"><span>Belongs to</span><span>Kind</span><span>Issued · last used</span><span>State</span><span></span></div>` : "") +
    body;
  syncKeyForm();
}

function syncKeyForm() {
  const kind = $("keyKind")?.value;
  $("keyName")?.classList.toggle("hidden", kind !== "smaster");
}

function showIssued(html) {
  const box = $("keyIssued");
  box.innerHTML = html;
  box.classList.remove("hidden");
}

async function issueKey(event) {
  event.preventDefault();
  const kind = $("keyKind").value;
  const body = { kind };
  if (kind === "smaster") {
    body.name = $("keyName").value.trim();
  }
  const gen = state.buildingGen;
  const r = await fetch(`${serverBase()}/keys`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body),
  });
  if (buildingStale(gen)) return;
  if (!r.ok) {
    const text = await r.text();
    if (!buildingStale(gen)) showIssued(`<span class="bad">${esc(text)}</span>`);
    return;
  }
  const k = await r.json();
  // Signed out before the key arrived: it is never shown to the next person.
  if (buildingStale(gen)) return;
  if (k.kind === "admission") {
    showIssued(`<b>${k.minted} admission rights issued.</b> ${k.available} can be used now. People ask to join from the sign-in page; you approve them above.`);
  } else {
    const who = k.kind === "smaster" ? (k.name.startsWith("yeni-") ? "for a new Smaster" : `for <b>${esc(k.name)}</b>`) : "to sign in as Master";
    const life = k.kind === "smaster"
      ? `They choose Smaster at sign-in and type their own name. Use within 7 days (by ${esc(keyWhen(k.use_by))}); once used it works until revoked.`
      : `Use it once, by ${esc(keyWhen(k.use_by))}. The session it opens lasts ${k.session_hours} hours. Any earlier unused Master key stopped working.`;
    showIssued(
      `<span class="keyonce">Shown once — copy it now; it will not be shown again.</span>` +
      `<b class="keyfor">Key ${who}</b>` +
      `<div class="keydim">${life} Send it to the person directly, never in a shared channel.</div>` +
      `<div class="keysecret"><code id="keySecret">${esc(k.code)}</code><button type="button" class="btn primary" data-key-copy>Copy</button></div>` +
      `<div class="keywarn">This is the only time it is shown. Never paste it into a loca: everyone at the table would read it.</div>` +
      `<div class="keydone"><button type="button" class="btn primary" data-key-done>I've sent it</button></div>`);
  }
  if (kind === "smaster") $("keyName").value = "";
  await fetchKeyLedger();
  if (!buildingStale(gen)) fetchPeople();
}

async function keyLedgerClick(event) {
  const filter = event.target.closest("[data-key-filter]");
  if (filter) { state.keyFilter = filter.dataset.keyFilter; renderKeyLedger(); return; }
  if (event.target.closest("[data-key-done]")) {
    const issued = $("keyIssued");
    issued.innerHTML = "";
    issued.classList.add("hidden");
    return;
  }
  if (event.target.closest("[data-key-copy]")) {
    const code = $("keySecret")?.textContent || "";
    const button = event.target.closest("[data-key-copy]");
    button.textContent = await copyText(code, $("keySecret")) ? "Copied" : "Selected — press Ctrl+C";
    return;
  }
  const revoke = event.target.closest("[data-key-revoke]");
  if (revoke) {
    if (!confirm(`Revoke ${revoke.dataset.keyLabel}? Whoever holds it can no longer use it.`)) return;
    const gen = state.buildingGen;
    const r = await fetch(`${serverBase()}/keys/${encodeURIComponent(revoke.dataset.keyRevoke)}`, {
      method: "DELETE",
      headers: adminHeaders({}),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const text = await r.text();
      if (buildingStale(gen)) return;
      showIssued(`<span class="bad">${esc(text)}</span>`);
    }
    await fetchKeyLedger();
    if (!buildingStale(gen)) fetchPeople();
  }
}

// The Master removes a Smaster from People › Ranks. A Smaster names
// themselves at sign-in, so the Master has no Rename here. The question is
// asked in words under the row; only "Yes, remove" sends DELETE /smasters/:id.
function removeSmasterAskHtml(rank) {
  const question = rank.unused
    ? `Delete the unused Smaster code <b>${esc(rank.name)}</b>? Nobody will be able to sign in with it.`
    : `Remove <b>${esc(rank.name)}</b> from the Building? Their Smaster key stops working, they are signed out, and floors they founded pass to the Master.`;
  return `<div class="rankconfirm" role="alertdialog" aria-label="Remove ${esc(rank.name)}"><span>${question}</span>` +
    `<span class="rankconfirmacts"><button type="button" class="btn" data-remove-smaster-no>No</button>` +
    `<button type="button" class="btn danger" data-remove-smaster-yes="${esc(rank.name)}">Yes, remove</button></span></div>`;
}

async function removeSmaster(name, button) {
  // The removal is the Master's who asked: never sent, and never reported,
  // under whoever signed in while the list was being read.
  const gen = state.buildingGen;
  button.disabled = true;
  const listed = await fetch(`${serverBase()}/smasters`, { headers: adminHeaders({}) });
  const list = listed.ok ? await listed.json() : [];
  if (buildingStale(gen)) return;
  const row = (Array.isArray(list) ? list : []).find(s => s.name === name);
  const r = row
    ? await fetch(`${serverBase()}/smasters/${encodeURIComponent(row.id)}`, { method: "DELETE", headers: adminHeaders({}) })
    : null;
  if (buildingStale(gen)) return;
  state.removingSmaster = null;
  if (r && !r.ok) { const why = await r.text(); if (!buildingStale(gen)) alert(why); }
  fetchPeople();
}

$("keyLedger").addEventListener("click", (event) => keyLedgerClick(event));
$("keyLedger").addEventListener("change", (event) => { if (event.target.id === "keyKind") syncKeyForm(); });
$("keyLedger").addEventListener("submit", (event) => issueKey(event));
$("peopleList").addEventListener("click", (event) => {
  const remove = event.target.closest("[data-remove-smaster]");
  if (remove) { state.removingSmaster = { name: remove.dataset.removeSmaster, gen: state.buildingGen }; renderPeople(); return; }
  if (event.target.closest("[data-remove-smaster-no]")) { state.removingSmaster = null; renderPeople(); return; }
  const yes = event.target.closest("[data-remove-smaster-yes]");
  if (yes) removeSmaster(yes.dataset.removeSmasterYes, yes);
});

// ---- Building sub-tabs: Members · Join requests · Floors · Keys · Private
// Groups is for the ranks (each sees only the groups it may see). Keys and
// Private messages are the Master's alone (the server answers 403 to anyone
// else); for everyone else those tabs are not drawn at all. Join requests is
// drawn once the list has been asked for (it is the Building admin's), and
// shows the Members pane with only its requests card: the card is one
// element under both tabs.
state.buildingTab = "people";

function joinRequestsKnown() {
  return isAdmin() && (Array.isArray(state.joinRequests) || state.joinRequestsError === true);
}

function syncBuildingTabs() {
  closePeopleExtrasOfAnother();
  const master = state.profile?.building_role === "master";
  const rank = master || state.profile?.building_role === "smaster";
  const requests = joinRequestsKnown();
  $("btabGroups").classList.toggle("hidden", !rank);
  $("btabKeys").classList.toggle("hidden", !master);
  $("btabPrivate").classList.toggle("hidden", !master);
  $("btabSettings").classList.toggle("hidden", !rank);
  $("btabRequests").classList.toggle("hidden", !requests);
  $("addPeopleBtn").classList.toggle("hidden", !isAdmin());
  syncJoinRequestsCount();
  if (!master && (state.buildingTab === "keys" || state.buildingTab === "private")) state.buildingTab = "people";
  if (!rank && (state.buildingTab === "groups" || state.buildingTab === "settings")) state.buildingTab = "people";
  if (!requests && state.buildingTab === "requests") state.buildingTab = "people";
  const panes = { people: "peopleList", requests: "peopleList", groups: "groupsPane", keys: "keyLedger", private: "dmOverview", settings: "buildingSettings" };
  const members = state.buildingTab === "people" || state.buildingTab === "requests";
  for (const [tab, pane] of Object.entries(panes)) {
    const on = state.buildingTab === tab;
    if (pane !== "peopleList") $(pane).classList.toggle("hidden", !on);
    const button = document.querySelector(`[data-btab="${tab}"]`);
    button.classList.toggle("active", on);
    button.setAttribute("aria-selected", String(on));
    button.tabIndex = on ? 0 : -1;
  }
  $("peopleList").classList.toggle("hidden", !members);
  $("peopleList").classList.toggle("jronly", state.buildingTab === "requests");
  $("peopleList").setAttribute("aria-labelledby", state.buildingTab === "requests" ? "btabRequests" : "btabPeople");
  if (!members) closeRowMenu(false);
}

function setBuildingTab(tab) {
  state.buildingTab = tab;
  syncBuildingTabs();
  if (tab === "keys") fetchKeyLedger();
  if (tab === "groups") fetchGroups();
  if (tab === "people" || tab === "requests") renderPeople();
  if (tab === "private") fetchDmOverview();
  if (tab === "settings") fetchBuildingSettings();
}

document.querySelector(".buildingtabs").addEventListener("click", (event) => {
  const tab = event.target.closest("[data-btab]");
  if (tab) setBuildingTab(tab.dataset.btab);
});
document.querySelector(".buildingtabs").addEventListener("keydown", (event) => {
  if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
  const tabs = [...document.querySelectorAll(".buildingtabs .btab:not(.hidden)")];
  const index = tabs.findIndex(t => t.dataset.btab === state.buildingTab);
  const next = tabs[(index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
  setBuildingTab(next.dataset.btab);
  next.focus();
});

// ---- Join requests tab, a row's ⋯, Add people -----------------------------
// The People boards (People.dc.html, AddPeople.dc.html) put a row's less
// common actions behind a ⋯ and bring people in from one "Add people"
// dialog. Both offer only what the server already does: seat a Building
// member in a loca (POST /rooms/{room}/call, as the sidebar's call list
// does), the join requests waiting, and admission rights (Mint 5).

// Whatever these hold belongs to the person signed in when they opened.
let peopleExtrasGen = null;
function closePeopleExtrasOfAnother() {
  if (peopleExtrasGen === state.buildingGen) return;
  peopleExtrasGen = state.buildingGen;
  closeRowMenu(false);
  closeAddPeople(false);
  $("peopleNotice").textContent = "";
}

function syncJoinRequestsCount() {
  const count = Array.isArray(state.joinRequests) ? state.joinRequests.length : 0;
  const badge = $("btabRequestsCount");
  badge.textContent = count ? String(count) : "";
  badge.classList.toggle("hidden", !count);
  badge.setAttribute("aria-label", count ? `${count} waiting` : "");
}

// renderPeople has drawn the list again (people.js).
function peopleViewDrawn() {
  syncBuildingTabs();
  syncRowMenu();
  if (!$("addPeople").classList.contains("hidden")) renderAddPeopleCounts();
}

// The locas someone can be seated in: open ones this viewer is listed, not
// the ones they already sit in, and on a floor only if they are on it (the
// server seats nobody else there).
function locasToSeat() {
  return (state.rooms || []).filter(rm => !rm.archived && !rm.special && !isDm(rm.room));
}
function canSeatIn(person, rm) {
  if (!person || person.suspended || (person.locas || []).includes(rm.room)) return false;
  if (!rm.group_id) return true;
  const floor = (state.groups || []).find(g => g.id === rm.group_id);
  return !!floor && (floor.members || []).some(m => m.name === person.name);
}

// Seat a Building member in a loca: the call the sidebar's call list makes
// (callIn), for a loca named here rather than the one open. Answers the line
// to show, or null when the person who asked has left.
async function seatInLoca(name, room) {
  const gen = state.buildingGen;
  const where = `#${locaName(room)}`;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/call`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name }),
    });
    if (buildingStale(gen)) return null;
    if (!r.ok) {
      const why = r.status === 404 ? "they are not a member of this building" : (await r.text().catch(() => "")) || `error ${r.status}`;
      return buildingStale(gen) ? null : { ok: false, text: `Could not invite ${name} to ${where}: ${why}` };
    }
    const v = await r.json().catch(() => ({}));
    if (buildingStale(gen)) return null;
    refreshRooms();
    if (room === state.room) { fetchLobby(); fetchSeated(); }
    await fetchPeople();
    if (buildingStale(gen)) return null;
    return { ok: true, text: v.already ? `${name} already has a seat in ${where}` : `${name} invited to ${where}` };
  } catch (e) {
    return buildingStale(gen) ? null : { ok: false, text: `Could not invite ${name} to ${where}: the server did not answer` };
  }
}

// ---- A row's ⋯ menu ------------------------------------------------------
// One menu for the page, placed under the ⋯ that opened it. The list is
// drawn again on every poll, so the menu remembers whose it is and finds
// that row's ⋯ again (syncRowMenu).
let rowMenuFor = null;

function rowMenuTrigger(name) {
  return [...$("peopleList").querySelectorAll("[data-row-menu]")].find(b => b.dataset.rowMenu === name) || null;
}

function rowMenuHtml(name) {
  const person = (state.people || []).find(p => p.name === name);
  const locas = locasToSeat().filter(rm => canSeatIn(person, rm));
  const items = locas.map(rm =>
    `<button type="button" class="prowitem" role="menuitem" tabindex="-1" data-row-seat="${esc(rm.room)}">#${esc(locaName(rm.room))}</button>`).join("");
  return `<div class="prowpophead" aria-hidden="true">Invite to a loca</div>` +
    (items || `<div class="prowpopnone">No other loca ${esc(name)} can enter</div>`);
}

function openRowMenu(name, trigger) {
  const menu = $("peopleRowMenu");
  if (rowMenuFor !== null) closeRowMenu(false);
  rowMenuFor = name;
  menu.innerHTML = rowMenuHtml(name);
  menu.setAttribute("aria-label", `More for ${name}`);
  menu.classList.remove("hidden");
  trigger.setAttribute("aria-expanded", "true");
  placeRowMenu(trigger);
  const first = menu.querySelector("[role=menuitem]");
  if (first) first.focus();
}

function closeRowMenu(returnFocus) {
  const menu = $("peopleRowMenu");
  if (!menu) return;
  const name = rowMenuFor;
  rowMenuFor = null;
  menu.classList.add("hidden");
  menu.innerHTML = "";
  if (name === null) return;
  const trigger = rowMenuTrigger(name);
  if (trigger) {
    trigger.setAttribute("aria-expanded", "false");
    if (returnFocus) trigger.focus();
  }
}

function placeRowMenu(trigger) {
  const menu = $("peopleRowMenu");
  const at = trigger.getBoundingClientRect();
  const width = menu.offsetWidth;
  const height = menu.offsetHeight;
  const left = Math.max(8, Math.min(at.right - width, window.innerWidth - width - 8));
  const below = at.bottom + 4;
  const top = below + height > window.innerHeight - 8 && at.top - height - 4 >= 8 ? at.top - height - 4 : below;
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;
}

function syncRowMenu() {
  if (rowMenuFor === null) return;
  const trigger = rowMenuTrigger(rowMenuFor);
  if (!trigger) { closeRowMenu(false); return; }
  trigger.setAttribute("aria-expanded", "true");
  placeRowMenu(trigger);
}

$("peopleList").addEventListener("click", (event) => {
  const trigger = event.target.closest("[data-row-menu]");
  if (!trigger) return;
  const name = trigger.dataset.rowMenu;
  if (rowMenuFor === name) closeRowMenu(true);
  else openRowMenu(name, trigger);
});

$("peopleRowMenu").addEventListener("click", async (event) => {
  const item = event.target.closest("[data-row-seat]");
  if (!item || rowMenuFor === null) return;
  const name = rowMenuFor;
  closeRowMenu(true);
  $("peopleNotice").textContent = `Inviting ${name}…`;
  const said = await seatInLoca(name, item.dataset.rowSeat);
  if (said) $("peopleNotice").textContent = said.text;
});

$("peopleRowMenu").addEventListener("keydown", (event) => {
  const items = [...$("peopleRowMenu").querySelectorAll("[role=menuitem]")];
  const index = items.indexOf(document.activeElement);
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeRowMenu(true); return; }
  if (event.key === "Tab") { closeRowMenu(false); return; }
  if (!items.length) return;
  const step = { ArrowDown: 1, ArrowUp: -1 }[event.key];
  if (step) {
    event.preventDefault();
    items[(index + step + items.length) % items.length].focus();
  } else if (event.key === "Home" || event.key === "End") {
    event.preventDefault();
    items[event.key === "Home" ? 0 : items.length - 1].focus();
  }
});

// A click elsewhere, a scroll or a resize puts the menu away.
document.addEventListener("pointerdown", (event) => {
  if (rowMenuFor === null) return;
  if (event.target.closest("#peopleRowMenu, [data-row-menu]")) return;
  closeRowMenu(false);
});
document.addEventListener("scroll", (event) => {
  if (rowMenuFor !== null && !$("peopleRowMenu").contains(event.target)) closeRowMenu(false);
}, true);
window.addEventListener("resize", () => { if (rowMenuFor !== null) closeRowMenu(false); });

// ---- Add people ------------------------------------------------------------
// Three things the Building does today: seat someone already in it in a loca,
// approve the join requests of people and agents who asked at the door, and
// mint the admission rights an approval spends.
state.addPeople = null;   // { room, query } while the dialog is open
let addPeopleReturnFocus = null;

function addPeopleRoom() {
  const locas = locasToSeat();
  const chosen = state.addPeople?.room;
  if (locas.some(rm => rm.room === chosen)) return chosen;
  if (locas.some(rm => rm.room === state.room)) return state.room;
  return locas[0]?.room || "";
}

function openAddPeople() {
  if (!isAdmin()) return;
  addPeopleReturnFocus = document.activeElement;
  closeRowMenu(false);
  state.addPeople = { room: "", query: "" };
  state.addPeople.room = addPeopleRoom();
  renderAddPeople();
  $("addPeople").classList.remove("hidden");
  setTimeout(() => ($("addPeopleSearch") || $("addPeopleClose")).focus(), 0);
}

function closeAddPeople(returnFocus = true) {
  const box = $("addPeople");
  if (!box) return;
  const wasOpen = !box.classList.contains("hidden");
  box.classList.add("hidden");
  $("addPeopleBody").innerHTML = "";
  state.addPeople = null;
  if (wasOpen && returnFocus && addPeopleReturnFocus && document.contains(addPeopleReturnFocus)) addPeopleReturnFocus.focus();
  addPeopleReturnFocus = null;
}

function renderAddPeople() {
  const room = state.addPeople.room;
  const locas = locasToSeat();
  const server = serverBase() || window.location.origin;
  const into = locas.length
    ? `<div class="aploca"><label for="addPeopleLoca">Into</label><select id="addPeopleLoca">` +
        locas.map(rm => `<option value="${esc(rm.room)}"${rm.room === room ? " selected" : ""}>#${esc(locaName(rm.room))}</option>`).join("") +
      `</select></div>` +
      `<label class="aplabel" for="addPeopleSearch">Search the building</label>` +
      `<input id="addPeopleSearch" type="search" placeholder="Search people and agents" autocomplete="off" spellcheck="false" value="${esc(state.addPeople.query)}" />` +
      `<div class="aplist" id="addPeopleList"></div>` +
      `<small class="aphint">An agent that is online enters at once; one that is away enters when it starts.</small>`
    : `<p class="apnone">There is no loca yet to invite anyone into.</p>`;
  $("addPeopleBody").innerHTML =
    `<section class="apsec apmain"><h3>Someone already in the building</h3>${into}</section>` +
    `<section class="apsec"><h3>Someone who asks to join</h3>` +
      `<p>A person asks from the sign-in page; an agent runs this. You approve them under Join requests.</p>` +
      `<pre class="apcmd">connect.sh request-join ${esc(server)} &lt;name&gt;</pre>` +
      `<div class="apline"><span id="addPeopleWaiting"></span>` +
      `<button type="button" class="btn" data-addp-requests="1">Open join requests</button></div></section>` +
    `<section class="apsec"><h3>Admission rights</h3>` +
      `<p>Each approved request spends one admission right.</p>` +
      `<div class="apline"><span id="addPeopleStock"></span>` +
      `<button type="button" class="btn" data-addp-mint="5" title="add five admission rights">Mint 5</button></div></section>` +
    `<div class="apnote" id="addPeopleNote" role="status" aria-live="polite"></div>`;
  renderAddPeopleList();
  renderAddPeopleCounts();
}

// Who can be seated in the chosen loca, matching the search.
function renderAddPeopleList() {
  const list = $("addPeopleList");
  if (!list || !state.addPeople) return;
  const rm = locasToSeat().find(r => r.room === state.addPeople.room);
  const q = state.addPeople.query.trim().toLowerCase();
  const free = (state.people || []).filter(p => rm && canSeatIn(p, rm))
    .filter(p => !q || p.name.toLowerCase().includes(q) || String(p.full_name || "").toLowerCase().includes(q))
    .sort((a, b) => a.name.localeCompare(b.name));
  if (!free.length) {
    const floor = rm?.group_id ? " on this loca's floor" : "";
    list.innerHTML = `<div class="apnone">${q ? "Nobody matches" : `Nobody else${floor} to invite here`}</div>`;
    return;
  }
  list.innerHTML = free.map(p => {
    const locas = p.locas || [];
    const where = (locas.length ? `in ${locas.map(l => "#" + locaName(l)).join(", ")}` : "waiting for a loca") +
      (p.online ? " · online" : " · away");
    return `<div class="aprow"><span class="apwho">${kindMark(p.kind === "agent" ? "agent" : "human")}<b>${esc(p.name)}</b></span>` +
      `<span class="apwhere">· ${esc(where)}</span>` +
      `<button type="button" class="btn primary" data-addp-invite="${esc(p.name)}">Invite</button></div>`;
  }).join("");
}

function renderAddPeopleCounts() {
  const waiting = $("addPeopleWaiting");
  if (!waiting) return;
  const jr = Array.isArray(state.joinRequests) ? state.joinRequests.length : 0;
  waiting.textContent = state.joinRequestsError && !jr ? "Join requests could not be loaded"
    : jr ? `${jr} waiting for an answer` : "Nobody is waiting";
  const open = document.querySelector("#addPeopleBody [data-addp-requests]");
  if (open) open.classList.toggle("hidden", $("btabRequests").classList.contains("hidden"));
  const stock = state.admissionStock;
  $("addPeopleStock").textContent = stock ? `${stock.available} of ${stock.total} left` : "Not known yet";
}

async function addPeopleInvite(button) {
  const name = button.dataset.addpInvite;
  const room = state.addPeople?.room;
  if (!room) return;
  button.disabled = true;
  $("addPeopleNote").textContent = `Inviting ${name}…`;
  const said = await seatInLoca(name, room);
  if (!said || !state.addPeople) return;
  button.disabled = false;
  $("addPeopleNote").textContent = said.text;
  renderAddPeopleList();
}

async function addPeopleMint(button) {
  const n = Number(button.dataset.addpMint) || 5;
  const gen = state.buildingGen;
  button.disabled = true;
  let note;
  try {
    const r = await fetch(`${serverBase()}/admission-stock`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ count: n }),
    });
    note = r.ok ? `${n} admission rights created` : (await r.text().catch(() => "")) || "could not mint";
  } catch (e) {
    note = "could not mint — network error, try again";
  }
  if (buildingStale(gen)) return;
  try { await fetchPeople(); } catch (e) { /* the counts stay as they were */ }
  if (buildingStale(gen) || !state.addPeople) return;
  button.disabled = false;
  renderAddPeopleCounts();
  $("addPeopleNote").textContent = note;
}

$("addPeopleBtn").onclick = () => openAddPeople();
$("addPeopleClose").onclick = () => closeAddPeople();
$("addPeopleBackdrop").onclick = () => closeAddPeople();
$("addPeopleBody").addEventListener("input", (event) => {
  if (event.target.id !== "addPeopleSearch" || !state.addPeople) return;
  state.addPeople.query = event.target.value;
  renderAddPeopleList();
});
$("addPeopleBody").addEventListener("change", (event) => {
  if (event.target.id !== "addPeopleLoca" || !state.addPeople) return;
  state.addPeople.room = event.target.value;
  $("addPeopleNote").textContent = "";
  renderAddPeopleList();
});
$("addPeopleBody").addEventListener("click", (event) => {
  const invite = event.target.closest("[data-addp-invite]");
  if (invite) { addPeopleInvite(invite); return; }
  const mint = event.target.closest("[data-addp-mint]");
  if (mint) { addPeopleMint(mint); return; }
  if (event.target.closest("[data-addp-requests]")) {
    closeAddPeople(false);
    setBuildingTab("requests");
    $("btabRequests").focus();
  }
});
document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  if (rowMenuFor !== null) { closeRowMenu(true); return; }
  if (!$("addPeople").classList.contains("hidden")) closeAddPeople();
});
