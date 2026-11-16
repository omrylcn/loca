"use strict";
// Groups (PRINCIPLES "Groups") in the Building view: found a group, rename,
// archive or delete it, and add or remove its people. The server shows each
// viewer only the groups it may see and refuses every act it may not do, so
// this file only stops offering what would be refused.

state.groups = [];
state.groupsLoaded = false;
state.groupFilter = "all";
state.profilesList = null;

// A floor is drawn as layers: storeys of the Building, not a crowd of people.
const GROUP_ICON = '<svg class="grpicon" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3 3 7.8l9 4.8 9-4.8z"/><path d="m3 12.4 9 4.8 9-4.8"/><path d="m3 17 9 4.8 9-4.8"/></svg>';
const GROUP_REMOVE_ICON = '<svg viewBox="0 0 24 24" width="11" height="11" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';

async function fetchGroups(render = true) {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/groups`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    if (r.ok) {
      const groups = await r.json();
      if (buildingStale(gen)) return;
      state.groups = groups; state.groupsLoaded = true;
    }
  } catch (e) { /* keep the last list; the health line owns transport errors */ }
  if (buildingStale(gen)) return;
  if (isAdmin() && !state.profilesList) await fetchProfilesList();
  if (buildingStale(gen)) return;
  syncBuildingTabs();
  if (!render) return;
  renderGroupsPane();
  if (state.tab === "people") renderPeople();
}

/// Every active person, for the add pickers. Building ranks only.
async function fetchProfilesList() {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/profiles`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    const list = r.ok ? await r.json() : [];
    if (buildingStale(gen)) return;
    state.profilesList = list;
  } catch (e) { if (!buildingStale(gen)) state.profilesList = []; }
}

/// The groups (visible to me) a person belongs to, by display name.
function groupsOfPerson(name) {
  return (state.groups || []).filter(g => (g.members || []).some(m => m.name === name));
}

function groupChipsHtml(name) {
  const mine = groupsOfPerson(name);
  if (!mine.length) return `<span class="pnowhere">ground floor</span>`;
  return mine.map(g => `<span class="grpchip" title="member of ${esc(g.name)}">${esc(g.name)}</span>`).join("");
}

/// Does a person pass the People tab's group filter?
function personInGroupFilter(name) {
  const f = state.groupFilter;
  if (!f || f === "all") return true;
  const mine = groupsOfPerson(name);
  if (f === "none") return !mine.length;
  return mine.some(g => g.id === f);
}

function groupFilterHtml() {
  const groups = state.groups || [];
  if (!groups.length) return "";
  const chips = [["all", "All"], ...groups.map(g => [g.id, g.name]), ["none", "Ground floor only"]];
  if (!chips.some(([v]) => v === state.groupFilter)) state.groupFilter = "all";
  return `<div class="keyfilters grpfilter" role="group" aria-label="Filter people by floor">` +
    chips.map(([v, l]) => `<button type="button" class="chip${state.groupFilter === v ? " on" : ""}" data-group-filter="${esc(v)}">${esc(l)}</button>`).join("") +
    `</div>`;
}

/// People the viewer may add to `g`: active, not already in it, never the
/// Master (it sees every group) and never the viewer (nobody adds themselves).
function groupCandidates(g) {
  const inGroup = new Set((g.members || []).map(m => m.principal_id));
  return (state.profilesList || []).filter(p =>
    p.role !== "master" && p.id !== state.principalId && !inGroup.has(p.id));
}

function groupOwnerLine(g) {
  return g.owned_by_master ? "owned by the Master" : `founded by ${esc(g.owner || "a former Smaster")}`;
}

function groupCardHtml(g) {
  const manage = !!g.can_manage;
  const archivedTag = g.archived ? `<span class="pflag warn">Archived</span>` : "";
  const acts = manage
    ? `<button type="button" class="btn" data-group-rename="${esc(g.id)}">Rename</button>` +
      `<button type="button" class="btn" data-group-archive="${esc(g.id)}" data-archived="${g.archived ? "0" : "1"}">${g.archived ? "Unarchive" : "Archive"}</button>` +
      `<button type="button" class="btn danger" data-group-delete="${esc(g.id)}">Delete</button>`
    : "";
  const locas = (g.locas || []).length
    ? g.locas.map(l => `<span class="locachip">#${esc(l)}</span>`).join("")
    : `<span class="pnowhere">no locas yet</span>`;
  const members = (g.members || []).map(m => {
    const kind = m.kind === "agent" ? "agent" : "human";
    const founder = !g.owned_by_master && m.principal_id === g.owner_id;
    const remove = manage && !founder
      ? `<button type="button" class="btn danger grpremove" data-group-remove="${esc(g.id)}" data-principal="${esc(m.principal_id)}" data-member-name="${esc(m.name)}" aria-label="Remove ${esc(m.name)} from ${esc(g.name)}" title="Remove from ${esc(g.name)}">${GROUP_REMOVE_ICON}<span>Remove</span></button>`
      : "";
    return `<div class="grpmember"><span class="pname">${kindMark(kind)}` +
      `<button type="button" class="pnamebtn" data-person-card="${esc(m.name)}">${esc(m.name)}</button>` +
      `${rankBadge(m.role)}${founder ? `<span class="pkind">founder</span>` : `<span class="pkind">${kind}</span>`}</span>` +
      `<span class="pacts">${remove}</span></div>`;
  }).join("") || `<div class="bbempty">No members yet.</div>`;
  const candidates = manage ? groupCandidates(g) : [];
  const add = manage
    ? `<div class="grpadd"><select data-group-add-select="${esc(g.id)}" aria-label="Person to add to ${esc(g.name)}">` +
        `<option value="">${candidates.length ? "Add a person…" : "Everyone is already in it"}</option>` +
        candidates.map(p => `<option value="${esc(p.id)}">${esc(p.display_name)} · ${p.kind === "agent" ? "agent" : p.role === "smaster" ? "Smaster" : "human"}</option>`).join("") +
      `</select><button type="button" class="btn primary" data-group-add="${esc(g.id)}"${candidates.length ? "" : " disabled"}>Add</button></div>`
    : "";
  return `<article class="grpcard${g.archived ? " archived" : ""}" data-group="${esc(g.id)}">` +
    `<div class="grphead"><span class="grpname">${GROUP_ICON}<b title="${esc(g.name)}">${esc(g.name)}</b>${archivedTag}</span>` +
      `<span class="pacts grpacts">${acts}</span>` +
      `<small class="grpowner">${groupOwnerLine(g)} · ${(g.members || []).length} ${(g.members || []).length === 1 ? "person" : "people"}</small></div>` +
    `<div class="grplocas"><span class="grplabel">Locas</span>${locas}</div>` +
    `<div class="grpmembers"><span class="grplabel">People</span>${members}</div>` +
    add + `</article>`;
}

function renderGroupsPane() {
  const box = $("groupsPane");
  if (!box) return;
  const keepNotice = $("groupNotice")?.textContent || "";
  const groups = state.groups || [];
  const form = isAdmin()
    ? `<form class="keyform grpform" id="groupForm" autocomplete="off">` +
        `<input id="groupName" maxlength="64" placeholder="new floor name, e.g. backend" aria-label="New floor name" />` +
        `<button type="submit" class="btn primary">Create floor</button></form>`
    : "";
  const list = groups.length
    ? groups.map(groupCardHtml).join("")
    : `<div class="bbempty">${state.groupsLoaded ? "No floors yet. Everyone is on the Ground floor." : "Loading floors…"}</div>`;
  box.innerHTML =
    `<div class="bbhead"><b>Floors</b>` +
    `<small>A floor's locas open only to its people. Everyone is on the Ground floor; the founder (or the Master) adds and removes people.</small></div>` +
    form + `<div class="grpnotice" id="groupNotice" role="status">${esc(keepNotice)}</div>` +
    `<div class="grplist">${list}</div>`;
}

/// `gen`: the generation the action began in; a notice from before a
/// sign-out is not shown to whoever signed in after it.
function groupNotice(text, gen = state.buildingGen) {
  const n = $("groupNotice");
  if (n && !buildingStale(gen)) n.textContent = text || "";
}

/// One write, then a fresh list everywhere it shows. Returns the error text
/// on failure ("" on success).
async function groupWrite(method, path, body) {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}${path}`, {
      method,
      headers: adminHeaders(body ? { "content-type": "application/json" } : {}),
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!r.ok) return (await r.text()) || `HTTP ${r.status}`;
  } catch (e) {
    return "could not reach the server — nothing was changed";
  }
  if (buildingStale(gen)) return "";
  await fetchGroups();
  if (typeof refreshRooms === "function") refreshRooms();
  return "";
}

async function createGroup(event) {
  event.preventDefault();
  const name = $("groupName").value.trim();
  if (!name) { $("groupName").focus(); return; }
  const gen = state.buildingGen;
  const error = await groupWrite("POST", "/groups", { name });
  if (buildingStale(gen)) return;
  groupNotice(error || `Floor ${name} created.`);
  if (!error) $("groupName").value = "";
}

async function addToGroup(groupId, principalId) {
  if (!principalId) return "pick a person first";
  return groupWrite("PUT", `/groups/${encodeURIComponent(groupId)}/members/${encodeURIComponent(principalId)}`);
}

async function removeFromGroup(groupId, principalId) {
  return groupWrite("DELETE", `/groups/${encodeURIComponent(groupId)}/members/${encodeURIComponent(principalId)}`);
}

function groupById(id) {
  return (state.groups || []).find(g => g.id === id);
}

async function groupsPaneClick(event) {
  const gen = state.buildingGen;
  const card = event.target.closest("[data-person-card]");
  if (card) { openPersonCard(card.dataset.personCard); return; }
  const rename = event.target.closest("[data-group-rename]");
  if (rename) {
    const g = groupById(rename.dataset.groupRename);
    const name = prompt("New name for this floor?", g?.name || "");
    if (!name || name.trim() === g?.name) return;
    const error = await groupWrite("PATCH", `/groups/${encodeURIComponent(rename.dataset.groupRename)}`, { name: name.trim() });
    groupNotice(error || "Renamed.", gen);
    return;
  }
  const archive = event.target.closest("[data-group-archive]");
  if (archive) {
    const archived = archive.dataset.archived === "1";
    const g = groupById(archive.dataset.groupArchive);
    if (archived && !confirm(`Archive ${g?.name}? Its locas close (read-only).`)) return;
    const error = await groupWrite("PATCH", `/groups/${encodeURIComponent(archive.dataset.groupArchive)}`, { archived });
    groupNotice(error || (archived ? "Archived; its locas are closed." : "Unarchived; its locas stay closed until reopened."), gen);
    return;
  }
  const del = event.target.closest("[data-group-delete]");
  if (del) {
    const g = groupById(del.dataset.groupDelete);
    if (!confirm(`Delete ${g?.name}? Its people are removed; closed locas stay with the Master.`)) return;
    const error = await groupWrite("DELETE", `/groups/${encodeURIComponent(del.dataset.groupDelete)}`);
    groupNotice(error || "Deleted.", gen);
    return;
  }
  const remove = event.target.closest("[data-group-remove]");
  if (remove) {
    const g = groupById(remove.dataset.groupRemove);
    if (!confirm(`Remove ${remove.dataset.memberName} from ${g?.name}? Their access to its locas ends now.`)) return;
    const error = await removeFromGroup(remove.dataset.groupRemove, remove.dataset.principal);
    groupNotice(error || `${remove.dataset.memberName} removed.`, gen);
    return;
  }
  const add = event.target.closest("[data-group-add]");
  if (add) {
    const select = add.parentElement.querySelector("select");
    const label = select?.selectedOptions[0]?.textContent?.split(" · ")[0] || "";
    const error = await addToGroup(add.dataset.groupAdd, select?.value);
    groupNotice(error || `${label} added.`, gen);
  }
}

$("groupsPane").addEventListener("click", (event) => groupsPaneClick(event));
$("groupsPane").addEventListener("submit", (event) => createGroup(event));
$("peopleList").addEventListener("click", (event) => {
  const filter = event.target.closest("[data-group-filter]");
  if (filter) { state.groupFilter = filter.dataset.groupFilter; renderPeople(); return; }
  const card = event.target.closest("[data-person-card]");
  if (card) openPersonCard(card.dataset.personCard);
});


// ---- A loca's group, under This Loca -----------------------------------
// The Master moves a loca between the default group and a group (or between
// groups). Before anything changes, the server says who would lose access,
// who is carried over by invite, and which titles end; the Master confirms.
state.groupMove = null;   // { room, target, preview, error, busy }

function currentLocaGroup() {
  const rm = (state.rooms || []).find(r => r.room === state.room);
  return rm ? { id: rm.group_id || "", name: rm.group || "" } : { id: "", name: "" };
}

function locaGroupFact() {
  if (!state.room || isDm(state.room) || state.room === state.homeRoom) return "";
  const g = currentLocaGroup();
  const master = state.profile?.building_role === "master";
  const change = master && !state.groupMove
    ? `<button type="button" class="extlink" data-loca-group-change>Change</button>` : "";
  return `<div><dt>Floor</dt><dd>${esc(g.name || "Ground floor")}${change}</dd></div>`;
}

function locaGroupMoveHtml() {
  const m = state.groupMove;
  if (!m || m.room !== state.room) return "";
  const current = currentLocaGroup().id;
  const options = [`<option value="">Ground floor</option>`]
    .concat((state.groups || []).filter(g => !g.archived)
      .map(g => `<option value="${esc(g.id)}"${m.target === g.id ? " selected" : ""}>${esc(g.name)}</option>`));
  let body = "";
  if (m.error) body = `<div class="gmerror">${esc(m.error)}</div>`;
  else if (m.preview) {
    const p = m.preview;
    const list = (title, names, cls) => names.length
      ? `<div class="gmline ${cls}"><b>${title}</b> ${names.map(esc).join(", ")}</div>` : "";
    body = list(`${p.losing.length} will lose access:`, p.losing, "bad") +
      list("Kept by invite:", p.carried_by_davet, "") +
      (p.lead_ends ? `<div class="gmline bad"><b>Lead ends:</b> ${esc(p.lead_ends)}</div>` : "") +
      (p.operator_ends ? `<div class="gmline bad"><b>Operator appointment ends:</b> ${esc(p.operator_ends)}</div>` : "") +
      (p.blocked ? `<div class="gmline bad"><b>Not enough seats:</b> ${Number(p.seats_needed)} people need an invite to stay, ${Number(p.seats_free)} seats are free. Release seats or remove people from the floor first.</div>` : "") +
      (!p.blocked && !p.losing.length && !p.lead_ends && !p.operator_ends ? `<div class="gmline">Nobody loses access.</div>` : "");
  } else if (m.target !== current) body = `<div class="gmline">checking…</div>`;
  const same = m.target === current;
  return `<div class="groupmove" role="group" aria-label="Move this loca to another floor">` +
    `<label class="gmfield"><span>Move to</span><select data-loca-group-target>${options.join("")}</select></label>` +
    (same ? `<div class="gmline">Choose another floor.</div>` : body) +
    `<div class="gmacts"><button type="button" class="btn" data-loca-group-cancel>Cancel</button>` +
    `<button type="button" class="btn primary" data-loca-group-confirm${same || !m.preview || m.preview.blocked || m.busy ? " disabled" : ""}>Move</button></div></div>`;
}

async function loadGroupMovePreview() {
  const m = state.groupMove;
  if (!m) return;
  m.preview = null; m.error = "";
  renderLocaSidebar();
  if (m.target === currentLocaGroup().id) return;
  try {
    const q = m.target ? `?group=${encodeURIComponent(m.target)}` : "";
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(m.room)}/group/preview${q}`, { headers: adminHeaders({}) });
    if (state.groupMove !== m) return;
    if (r.ok) m.preview = await r.json(); else m.error = await r.text();
  } catch (e) {
    m.error = "could not reach the server";
  }
  // Sign-out drops state.groupMove: a stale answer lands on nothing.
  if (state.groupMove !== m) return;
  renderLocaSidebar();
}

async function locaGroupClick(event) {
  const gen = state.buildingGen;
  if (event.target.closest("[data-loca-group-change]")) {
    if (!state.groupsLoaded) await fetchGroups(false);
    if (buildingStale(gen)) return;
    state.groupMove = { room: state.room, target: currentLocaGroup().id, preview: null, error: "", busy: false };
    renderLocaSidebar();
    return;
  }
  if (event.target.closest("[data-loca-group-cancel]")) { state.groupMove = null; renderLocaSidebar(); return; }
  if (event.target.closest("[data-loca-group-confirm]")) {
    const m = state.groupMove;
    if (!m || !m.preview || m.preview.blocked) return;
    m.busy = true; renderLocaSidebar();
    const err = await groupWrite("PUT", `/rooms/${encodeURIComponent(m.room)}/group`, { group: m.target || null });
    if (buildingStale(gen)) return;
    if (err) { m.busy = false; m.error = err; renderLocaSidebar(); return; }
    state.groupMove = null;
    renderLocaSidebar();
  }
}

$("locaSummary").addEventListener("click", (event) => locaGroupClick(event));
$("locaSummary").addEventListener("change", (event) => {
  const sel = event.target.closest("[data-loca-group-target]");
  if (!sel || !state.groupMove) return;
  state.groupMove.target = sel.value;
  loadGroupMovePreview();
});
