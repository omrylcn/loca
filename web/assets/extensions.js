"use strict";
// Loca extensions (PRINCIPLES "Loca extensions"). Chat is always on; the rest
// is chosen per loca: on the create screen (`+`) and under This Loca ›
// Extensions. The server refuses what is off — this file only stops offering
// it: tabs and the live toggle follow the loca's set, and data for an
// extension that is off is never fetched.

// A loca saved before extensions existed has no `extensions` field and keeps
// every one it already had. Frozen, like the server's LEGACY_EXTENSIONS.
const LEGACY_EXTENSIONS = ["notes", "focus", "journal", "wiki", "live"];
const EXTENSION_TABS = { notes: "tabNotes", focus: "tabTasks", journal: "tabJournal", wiki: "tabWiki" };
const EXTENSION_TAB_NAMES = { notes: "notes", focus: "tasks", journal: "journal", wiki: "wiki" };
const EXT_CHECK_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5 9-10"/></svg>';
const EXT_PLUS_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>';

let extensionCatalogCache = null;
let extensionRoomFetchSequence = 0;
let extensionsAppliedFor = { room: null, enabled: null };

async function extensionCatalog() {
  if (extensionCatalogCache) return extensionCatalogCache;
  const r = await fetch(`${serverBase()}/extensions`, { headers: adminHeaders({}) });
  if (!r.ok) throw new Error(await r.text() || `HTTP ${r.status}`);
  extensionCatalogCache = await r.json();
  return extensionCatalogCache;
}

/// The enabled set of the current loca, or null while its settings load.
function currentExtensions() {
  if (!state.room) return [];
  if (isBirebir(state.room)) return [];
  if (!isLocaContextReady("settings")) return null;
  const list = state.settings?.extensions;
  return Array.isArray(list) ? list : LEGACY_EXTENSIONS;
}

/// Whether an extension is on here. While settings load, nothing is assumed
/// on, so no request for a switched-off extension ever leaves the page.
function extensionOn(id) {
  const list = currentExtensions();
  return !!list && list.includes(id);
}

function extensionName(id) {
  const spec = extensionCatalogCache?.catalog?.find(entry => entry.id === id);
  return spec?.name || { notes: "Notes", focus: "Focus", journal: "Journal", wiki: "Loca Wiki", live: "Live" }[id] || id;
}

/// Tabs, the live toggle and the This Loca block follow the set. Called from
/// renderSettings, so every settings frame (a change by someone else too)
/// lands here.
function applyLocaExtensions() {
  const list = currentExtensions();
  for (const [id, tabId] of Object.entries(EXTENSION_TABS)) {
    $(tabId).classList.toggle("hidden", !list || !list.includes(id));
  }
  if (list && !list.includes("live")) {
    $("liveToggle").classList.add("hidden");
    $("liveBadge").classList.add("hidden");
  }
  const current = Object.entries(EXTENSION_TAB_NAMES).find(([, tab]) => tab === state.tab);
  if (list && current && !list.includes(current[0])) switchTab("chat");
  // An extension switched on while we are here: load what it holds.
  if (list && extensionsAppliedFor.room === state.room && extensionsAppliedFor.enabled) {
    const added = list.filter(id => !extensionsAppliedFor.enabled.includes(id));
    if (added.includes("notes")) fetchNotes();
    if (added.includes("focus")) fetchTasks();
    if (added.includes("journal")) fetchJournal();
  }
  extensionsAppliedFor = { room: state.room, enabled: list ? [...list] : null };
}

/// The Journal, Notes and Focus data for a just-joined loca, once its
/// settings say which of them exist here.
async function fetchExtensionData() {
  const room = state.room;
  await fetchSettings();
  if (room !== state.room) return;
  if (extensionOn("notes")) fetchNotes();
  fetchTasks();
  if (extensionOn("journal")) fetchJournal();
  if (state.tab === "wiki" && extensionOn("wiki")) fetchWiki();
}

async function fetchLocaExtensions() {
  const room = state.room;
  const sequence = ++extensionRoomFetchSequence;
  state.locaExtensions = null;
  if (!room || isBirebir(room)) return;
  try {
    const [catalog, response] = await Promise.all([
      extensionCatalog().catch(() => null),
      fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/extensions`, { headers: adminHeaders({}) }),
    ]);
    const view = response.ok ? await response.json() : null;
    if (sequence !== extensionRoomFetchSequence || room !== state.room) return;
    state.locaExtensions = view;
    if (!catalog && view?.catalog) extensionCatalogCache = { catalog: view.catalog, defaults: [] };
  } catch (error) {
    if (sequence !== extensionRoomFetchSequence || room !== state.room) return;
    state.locaExtensions = null;
  }
  renderLocaSidebar();
  if (!$("extManage").classList.contains("hidden")) renderExtensionManage();
}

/// The EXTENSIONS block under This Loca: chips for what is on, and a link to
/// the list (Manage for those who may change it, View for everyone else).
function locaExtensionsBlock() {
  if (!state.room || isBirebir(state.room)) return "";
  const list = currentExtensions();
  const chips = !list ? `<span class="extnone">Loading…</span>`
    : list.length ? list.map(id => `<span class="extchip">${esc(extensionName(id))}</span>`).join("")
    : `<span class="extnone">Chat only</span>`;
  const canManage = !!state.locaExtensions?.can_manage;
  const link = state.locaExtensions
    ? `<button type="button" class="extlink" id="extManageOpen" aria-haspopup="dialog">${canManage ? "Manage" : "View"}</button>`
    : "";
  return `<div class="locaext"><div class="locaexthead"><span>Extensions</span>${link}</div>` +
    `<div class="extchips" id="extChips">${chips}</div></div>`;
}

/* ---- dialogs ---- */
let extDialogReturnFocus = null;
function openExtDialog(id, focusId) {
  extDialogReturnFocus = document.activeElement;
  $(id).classList.remove("hidden");
  setTimeout(() => $(focusId)?.focus(), 0);
}
function closeExtDialog(id) {
  $(id).classList.add("hidden");
  if (extDialogReturnFocus && document.contains(extDialogReturnFocus)) extDialogReturnFocus.focus();
  extDialogReturnFocus = null;
}

function extensionRow(spec, checked) {
  return `<label class="extrow"><input type="checkbox" value="${esc(spec.id)}"${checked ? " checked" : ""}>` +
    `<span class="extrowtext"><b>${esc(spec.name)}</b><small>${esc(spec.summary)}</small></span></label>`;
}

function renderCreateTabsLine() {
  const ticked = [...document.querySelectorAll("#extCreateDefault input:checked, #extCreateAdd input:checked")]
    .map(input => input.value);
  const catalog = extensionCatalogCache?.catalog || [];
  const tabs = ["Chat", ...catalog.filter(spec => spec.tab && ticked.includes(spec.id)).map(spec => spec.tab)];
  const live = ticked.includes("live") ? " + live toggle" : "";
  $("extCreateTabs").innerHTML = `Tabs in this loca: <b>${tabs.map(esc).join(" · ")}</b>${esc(live)}`;
}

async function openCreateLoca(name = "") {
  $("extCreateName").value = name;
  $("extCreateError").textContent = "";
  $("extCreateSubmit").disabled = false;
  openExtDialog("extCreate", name ? "extCreateSubmit" : "extCreateName");
  try {
    const { catalog, defaults } = await extensionCatalog();
    $("extCreateDefault").innerHTML =
      `<label class="extrow fixed"><input type="checkbox" checked disabled>` +
        `<span class="extrowtext"><b>Chat</b><small>Always on. Not an extension.</small></span></label>` +
      catalog.filter(spec => defaults.includes(spec.id)).map(spec => extensionRow(spec, true)).join("");
    $("extCreateAdd").innerHTML =
      catalog.filter(spec => !defaults.includes(spec.id)).map(spec => extensionRow(spec, false)).join("");
    renderCreateTabsLine();
  } catch (error) {
    $("extCreateError").textContent = `Could not load the extension list: ${error.message}`;
  }
}

async function submitCreateLoca(event) {
  event?.preventDefault();
  const name = $("extCreateName").value.trim();
  if (!name) { $("extCreateError").textContent = "A loca needs a name."; $("extCreateName").focus(); return; }
  const extensions = [...document.querySelectorAll("#extCreateDefault input:checked:not(:disabled), #extCreateAdd input:checked")]
    .map(input => input.value);
  $("extCreateSubmit").disabled = true;
  $("extCreateError").textContent = "";
  try {
    const response = await fetch(`${serverBase()}/rooms`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name, extensions }),
    });
    if (!response.ok) {
      $("extCreateError").textContent = `Could not create loca: ${await response.text()}`;
      $("extCreateSubmit").disabled = false;
      return;
    }
  } catch (error) {
    $("extCreateError").textContent = "Loca creation could not be confirmed. Refresh Your Locas before retrying.";
    $("extCreateSubmit").disabled = false;
    return;
  }
  closeExtDialog("extCreate");
  $("newRoomName").value = "";
  joinRoom(name);
}

function renderExtensionManage() {
  const view = state.locaExtensions;
  const catalog = view?.catalog || extensionCatalogCache?.catalog || [];
  const enabled = currentExtensions() || [];
  const canManage = !!view?.can_manage;
  $("extManageRoom").textContent = roomLabel(state.room);
  $("extManageNote").textContent = canManage
    ? "Removing an extension hides its tab and the server refuses it. Its records are kept and come back when it is added again."
    : "Only this loca's operator, the Master or a Smaster changes its extensions.";
  const rows = catalog.map(spec => {
    const on = enabled.includes(spec.id);
    const action = canManage
      ? (on
        ? `<button type="button" class="btn extadded" data-ext-remove="${esc(spec.id)}" aria-pressed="true" title="Remove ${esc(spec.name)}">${EXT_CHECK_ICON}<span>Added</span></button>`
        : `<button type="button" class="btn primary" data-ext-add="${esc(spec.id)}" aria-pressed="false" title="Add ${esc(spec.name)}">${EXT_PLUS_ICON}<span>Add</span></button>`)
      : `<span class="extstate${on ? " on" : ""}">${on ? "On" : "Off"}</span>`;
    return `<div class="extitem${on ? " on" : ""}" data-ext="${esc(spec.id)}"><span class="extrowtext"><b>${esc(spec.name)}</b>` +
      `<small>${esc(spec.summary)}</small></span>${action}</div>`;
  });
  $("extManageList").innerHTML =
    `<div class="extitem on fixed"><span class="extrowtext"><b>Chat</b><small>Always on. Not an extension.</small></span>` +
      `<span class="extstate on">Always</span></div>` + rows.join("");
}

function openManageExtensions() {
  $("extManageError").textContent = "";
  renderExtensionManage();
  openExtDialog("extManage", "extManageClose");
}

async function changeExtension(id, add) {
  const room = state.room;
  $("extManageError").textContent = "";
  for (const button of document.querySelectorAll("#extManageList button")) button.disabled = true;
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/extensions/${encodeURIComponent(id)}`, {
      method: add ? "PUT" : "DELETE", headers: adminHeaders({}),
    });
    if (!response.ok) {
      $("extManageError").textContent = await response.text() || `HTTP ${response.status}`;
    } else if (room === state.room) {
      const view = await response.json();
      state.locaExtensions = view;
      // The settings frame follows; adopt the new set now so the list does
      // not flicker back while it travels.
      state.settings = { ...(state.settings || {}), extensions: view.enabled };
      renderSettings();
    }
  } catch (error) {
    $("extManageError").textContent = "Could not reach the server. Nothing was changed.";
  }
  if (room === state.room) renderExtensionManage();
}

$("extCreateForm").onsubmit = (e) => submitCreateLoca(e);
$("extCreateCancel").onclick = () => closeExtDialog("extCreate");
$("extCreateBackdrop").onclick = () => closeExtDialog("extCreate");
$("extCreate").addEventListener("change", (e) => { if (e.target.matches("input[type=checkbox]")) renderCreateTabsLine(); });
$("extManageClose").onclick = () => closeExtDialog("extManage");
$("extManageBackdrop").onclick = () => closeExtDialog("extManage");
$("extManageList").onclick = (e) => {
  const add = e.target.closest("[data-ext-add]");
  const remove = e.target.closest("[data-ext-remove]");
  if (add) changeExtension(add.dataset.extAdd, true);
  else if (remove) changeExtension(remove.dataset.extRemove, false);
};
$("locaSummary").addEventListener("click", (e) => { if (e.target.closest("#extManageOpen")) openManageExtensions(); });
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!$("extCreate").classList.contains("hidden")) closeExtDialog("extCreate");
  else if (!$("extManage").classList.contains("hidden")) closeExtDialog("extManage");
});
