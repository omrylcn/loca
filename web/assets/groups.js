"use strict";
// Groups (PRINCIPLES "Groups") in the Building view: found a group, rename,
// archive or delete it, and add or remove its people. The server shows each
// viewer only the groups it may see and refuses every act it may not do, so
// this file only stops offering what would be refused.

state.groups = [];
state.groupsLoaded = false;
state.groupFilter = "all";
state.profilesList = null;

const GROUP_ICON = '<svg class="grpicon" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="8" r="3"/><path d="M3 19c0-3.3 2.7-5 6-5s6 1.7 6 5"/><circle cx="17" cy="9" r="2.4"/><path d="M16 14.2c2.9.1 5 1.6 5 4.8"/></svg>';
const GROUP_REMOVE_ICON = '<svg viewBox="0 0 24 24" width="11" height="11" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';

async function fetchGroups(render = true) {
  try {
    const r = await fetch(`${serverBase()}/groups`, { headers: adminHeaders({}) });
    if (r.ok) { state.groups = await r.json(); state.groupsLoaded = true; }
  } catch (e) { /* keep the last list; the health line owns transport errors */ }
  if (isAdmin() && !state.profilesList) await fetchProfilesList();
  syncBuildingTabs();
  if (!render) return;
  renderGroupsPane();
  if (state.tab === "people") renderPeople();
}

/// Every active person, for the add pickers. Building ranks only.
async function fetchProfilesList() {
  try {
    const r = await fetch(`${serverBase()}/profiles`, { headers: adminHeaders({}) });
    state.profilesList = r.ok ? await r.json() : [];
  } catch (e) { state.profilesList = []; }
}

/// The groups (visible to me) a person belongs to, by display name.
function groupsOfPerson(name) {
  return (state.groups || []).filter(g => (g.members || []).some(m => m.name === name));
}

function groupChipsHtml(name) {
  const mine = groupsOfPerson(name);
  if (!mine.length) return `<span class="pnowhere">default</span>`;
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
  const chips = [["all", "All"], ...groups.map(g => [g.id, g.name]), ["none", "No group"]];
  if (!chips.some(([v]) => v === state.groupFilter)) state.groupFilter = "all";
  return `<div class="keyfilters grpfilter" role="group" aria-label="Filter people by group">` +
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
    `<div class="grphead"><span class="grpname">${GROUP_ICON}<b>${esc(g.name)}</b>${archivedTag}</span>` +
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
        `<input id="groupName" maxlength="64" placeholder="new group name, e.g. backend" aria-label="New group name" />` +
        `<button type="submit" class="btn primary">Create group</button></form>`
    : "";
  const list = groups.length
    ? groups.map(groupCardHtml).join("")
    : `<div class="bbempty">${state.groupsLoaded ? "No groups yet. Everyone is in the default group." : "Loading groups…"}</div>`;
  box.innerHTML =
    `<div class="bbhead"><b>Groups</b>` +
    `<small>A group's locas open only to its people. Everyone is in the default group; the founder (or the Master) adds and removes people.</small></div>` +
    form + `<div class="grpnotice" id="groupNotice" role="status">${esc(keepNotice)}</div>` +
    `<div class="grplist">${list}</div>`;
}

function groupNotice(text) {
  const n = $("groupNotice");
  if (n) n.textContent = text || "";
}

/// One write, then a fresh list everywhere it shows. Returns the error text
/// on failure ("" on success).
async function groupWrite(method, path, body) {
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
  await fetchGroups();
  if (typeof refreshRooms === "function") refreshRooms();
  return "";
}

async function createGroup(event) {
  event.preventDefault();
  const name = $("groupName").value.trim();
  if (!name) { $("groupName").focus(); return; }
  const error = await groupWrite("POST", "/groups", { name });
  groupNotice(error || `Group ${name} created.`);
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
  const card = event.target.closest("[data-person-card]");
  if (card) { openPersonCard(card.dataset.personCard); return; }
  const rename = event.target.closest("[data-group-rename]");
  if (rename) {
    const g = groupById(rename.dataset.groupRename);
    const name = prompt("New name for this group?", g?.name || "");
    if (!name || name.trim() === g?.name) return;
    const error = await groupWrite("PATCH", `/groups/${encodeURIComponent(rename.dataset.groupRename)}`, { name: name.trim() });
    groupNotice(error || "Renamed.");
    return;
  }
  const archive = event.target.closest("[data-group-archive]");
  if (archive) {
    const archived = archive.dataset.archived === "1";
    const g = groupById(archive.dataset.groupArchive);
    if (archived && !confirm(`Archive ${g?.name}? Its locas close (read-only).`)) return;
    const error = await groupWrite("PATCH", `/groups/${encodeURIComponent(archive.dataset.groupArchive)}`, { archived });
    groupNotice(error || (archived ? "Archived; its locas are closed." : "Unarchived; its locas stay closed until reopened."));
    return;
  }
  const del = event.target.closest("[data-group-delete]");
  if (del) {
    const g = groupById(del.dataset.groupDelete);
    if (!confirm(`Delete ${g?.name}? Its people are removed; closed locas stay with the Master.`)) return;
    const error = await groupWrite("DELETE", `/groups/${encodeURIComponent(del.dataset.groupDelete)}`);
    groupNotice(error || "Deleted.");
    return;
  }
  const remove = event.target.closest("[data-group-remove]");
  if (remove) {
    const g = groupById(remove.dataset.groupRemove);
    if (!confirm(`Remove ${remove.dataset.memberName} from ${g?.name}? Their access to its locas ends now.`)) return;
    const error = await removeFromGroup(remove.dataset.groupRemove, remove.dataset.principal);
    groupNotice(error || `${remove.dataset.memberName} removed.`);
    return;
  }
  const add = event.target.closest("[data-group-add]");
  if (add) {
    const select = add.parentElement.querySelector("select");
    const label = select?.selectedOptions[0]?.textContent?.split(" · ")[0] || "";
    const error = await addToGroup(add.dataset.groupAdd, select?.value);
    groupNotice(error || `${label} added.`);
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
