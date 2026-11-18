"use strict";
// Loca extensions (PRINCIPLES "Loca extensions"). Chat is always on; the rest
// is chosen per loca: on the create screen (`+`) and under This Loca ›
// Extensions. The server refuses what is off — this file only stops offering
// it: tabs and the live toggle follow the loca's set, and data for an
// extension that is off is never fetched. Which extensions exist, their names
// and which tab each one shows (`tab_key`, the tab's `data-tab`) come from
// the server's catalog; this file keeps no list of its own.

// A loca saved before extensions existed has no `extensions` field and keeps
// every one it already had. Frozen, like the server's LEGACY_EXTENSIONS.
const LEGACY_EXTENSIONS = ["notes", "focus", "journal", "wiki", "live"];
const EXT_CHECK_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5 9-10"/></svg>';
const EXT_PLUS_ICON = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>';

let extensionCatalogCache = null;
let extensionRoomFetchSequence = 0;
// A catalog that could not be read is asked for again on its own: after
// these pauses, for the loca it was asked for, one attempt at a time (an
// attempt is the catalog and the loca's view, read together). Then This Loca
// says so and offers to try again.
const EXTENSION_CATALOG_RETRY_MS = [1000, 2000, 4000];
let extensionCatalogRetry = { timer: null, exhausted: false };
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
  if (isDm(state.room)) return [];
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

/// The server's catalog, or null until it has been read.
function extensionCatalogList() {
  return extensionCatalogCache?.catalog || state.locaExtensions?.catalog || null;
}

/// The catalog entry for `id`, or null while the catalog is unknown.
function extensionSpec(id) {
  return extensionCatalogList()?.find(entry => entry.id === id) || null;
}

/// The extension whose tab is `.tabs [data-tab=key]`, from the catalog.
function extensionOfTab(key) {
  return extensionCatalogList()?.find(entry => entry.tab_key && entry.tab_key === key) || null;
}

function extensionName(id) {
  return extensionSpec(id)?.name || id;
}

/// Tabs, the live toggle and the This Loca block follow the set. Called from
/// renderSettings, so every settings frame (a change by someone else too)
/// lands here, and again when the catalog arrives.
function applyLocaExtensions() {
  const list = currentExtensions();
  // Every loca tab but Chat belongs to the extension whose `tab_key` names
  // it. It shows only once both the catalog and this loca's set say it is on.
  for (const tab of document.querySelectorAll(".tabs [role=tab]")) {
    if (tab.dataset.tab === "chat") continue;
    const spec = extensionOfTab(tab.dataset.tab);
    const on = !!list && !!spec && list.includes(spec.id);
    tab.classList.toggle("hidden", !on && !continuityShowsTab(tab.dataset.tab));
  }
  if (list && !list.includes("live")) {
    $("liveToggle").classList.add("hidden");
    $("liveBadge").classList.add("hidden");
  }
  const current = extensionOfTab(state.tab);
  if (list && current && !list.includes(current.id) && !continuityShowsTab(state.tab)) switchTab("chat");
  // An extension switched on while we are here: load what it holds.
  if (list && extensionsAppliedFor.room === state.room && extensionsAppliedFor.enabled) {
    const added = list.filter(id => !extensionsAppliedFor.enabled.includes(id));
    const removed = extensionsAppliedFor.enabled.filter(id => !list.includes(id));
    if (added.includes("board") || removed.includes("board")) boardSetChanged();
    if (added.includes("notes")) fetchNotes();
    if (added.includes("focus")) fetchTasks();
    if (added.includes("journal")) fetchJournal();
  }
  extensionsAppliedFor = { room: state.room, enabled: list ? [...list] : null };
  refreshContinuityTab();
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
  if (state.tab === "vault" && extensionOn("vault")) fetchVault();
}

async function fetchLocaExtensions(attempt = 0) {
  const room = state.room;
  const sequence = ++extensionRoomFetchSequence;
  const gen = state.buildingGen;
  clearTimeout(extensionCatalogRetry.timer);
  extensionCatalogRetry = { timer: null, exhausted: false };
  state.locaExtensions = null;
  if (!room || isDm(room)) return;
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
  if (!extensionCatalogList()) {
    const pause = EXTENSION_CATALOG_RETRY_MS[attempt];
    if (pause === undefined) {
      extensionCatalogRetry.exhausted = true;
    } else {
      // Not for another loca, another join of this one, or after sign-out.
      extensionCatalogRetry.timer = setTimeout(() => {
        if (sequence !== extensionRoomFetchSequence || room !== state.room || buildingStale(gen)) return;
        fetchLocaExtensions(attempt + 1);
      }, pause);
    }
  }
  // The tabs may have been laid out from the settings before the catalog
  // that names them was here.
  applyLocaExtensions();
  renderLocaSidebar();
  if (!$("extManage").classList.contains("hidden")) renderExtensionManage();
}

/// The EXTENSIONS block under This Loca: chips for what is on, and a link to
/// the list (Manage for those who may change it, View for everyone else).
function locaExtensionsBlock() {
  if (!state.room || isDm(state.room)) return "";
  const list = currentExtensions();
  const unknown = !list || (list.length && !extensionCatalogList());
  const chips = unknown && list && extensionCatalogRetry.exhausted
    ? `<span class="extnone" id="extUnavailable">Could not load the extension list.</span>` +
      `<button type="button" class="extlink" id="extRetry">Try again</button>`
    : unknown ? `<span class="extnone">Loading…</span>`
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

/// The Group choice: Default, or a group this person may open a loca in
/// (the Master, or a Smaster who is a member). Hidden when there is none.
async function renderCreateGroupChoice(preferred = "") {
  const field = $("extCreateGroupField");
  const select = $("extCreateGroup");
  if (isAdmin() && typeof fetchGroups === "function") await fetchGroups(false);
  const open = (state.groups || []).filter(g => g.can_create_loca);
  select.innerHTML = `<option value="">Ground floor — by invitation</option>` +
    open.map(g => `<option value="${esc(g.id)}">${esc(g.name)}</option>`).join("");
  if (open.some(g => g.id === preferred)) select.value = preferred;
  field.classList.toggle("hidden", !open.length);
  syncCreateGroupNote();
}

function syncCreateGroupNote() {
  const id = $("extCreateGroup").value;
  const g = (state.groups || []).find(group => group.id === id);
  $("extCreateGroupNote").textContent = g
    ? `Only ${g.name}'s people and the Master will see and enter it.`
    : "Anyone you invite or call in can enter it.";
}

/// A new loca's name is ASCII letters, digits, ".", "_" and "-", starting
/// with a letter or digit: it is the loca's address and its agents' DAVET_
/// key (the server's rule, crates/server/src/hub/loca_name.rs). A space
/// becomes "-" as it is typed; anything else the server would refuse stays
/// in the field and is named under it before Create. A display name with
/// spaces is the loca's title, given once it is open (this panel takes no
/// title when it opens a loca).
let createNameSpaced = false;
const TITLE_LATER = "A name with spaces or other letters can be the loca's title once it is open: This Loca › Loca identity.";

function locaNameProblem(name) {
  const bad = [...new Set([...name].filter(c => !/[A-Za-z0-9._-]/.test(c)))];
  if (bad.length) return `Not allowed in a name: ${bad.map(c => `“${c}”`).join(" ")}. Use letters a–z, digits, “.”, “_” or “-”.`;
  if (name && !/^[A-Za-z0-9]/.test(name)) return "A name starts with a letter or a digit.";
  return "";
}

function syncCreateNameHint() {
  const hint = $("extCreateNameHint");
  const name = $("extCreateName").value;
  const problem = locaNameProblem(name);
  const note = problem ? `${problem} ${TITLE_LATER}`
    : createNameSpaced && name ? `Spaces become “-”. ${TITLE_LATER}` : "";
  hint.textContent = note;
  hint.classList.toggle("bad", !!problem);
  hint.style.color = problem ? "var(--bad)" : "";
  hint.classList.toggle("hidden", !note || locaPanelMode !== "create");
}

$("extCreateName").addEventListener("input", (e) => {
  if (locaPanelMode !== "create") return;
  const field = e.target;
  if (field.value.includes(" ")) {
    const { selectionStart, selectionEnd } = field;
    field.value = field.value.replace(/ /g, "-");
    field.setSelectionRange(selectionStart, selectionEnd);
    createNameSpaced = true;
  }
  syncCreateNameHint();
});

async function openCreateLoca(name = "", group = "") {
  createNameSpaced = name.includes(" ");
  $("extCreateName").value = name.replace(/ /g, "-");
  $("extCreateError").textContent = "";
  $("extCreateSubmit").disabled = false;
  resetIdentityForCreate();
  syncCreateNameHint();
  openExtDialog("extCreate", name ? "extCreateSubmit" : "extCreateName");
  renderCreateGroupChoice(group);
  const gen = state.buildingGen;
  try {
    const { catalog, defaults } = await extensionCatalog();
    $("extCreateDefault").innerHTML =
      `<label class="extrow fixed"><input type="checkbox" checked disabled>` +
        `<span class="extrowtext"><b>Chat</b><small>Always on. Not an extension.</small></span></label>` +
      catalog.filter(spec => defaults.includes(spec.id)).map(spec => extensionRow(spec, createPreset() === "loca")).join("");
    // The Loca Board needs a Lead, which a loca being opened does not have
    // yet (the server refuses it here); it is turned on later, under Manage.
    $("extCreateAdd").innerHTML =
      catalog.filter(spec => !defaults.includes(spec.id) && spec.id !== "board")
        .map(spec => extensionRow(spec, false)).join("");
    renderCreateTabsLine();
  } catch (error) {
    if (!buildingStale(gen)) $("extCreateError").textContent = `Could not load the extension list: ${error.message}`;
  }
}

/// The create panel's preset (identity.js) ticks the extensions: the
/// catalog's defaults for a Loca, none for a Lounge. Ticks stay changeable.
function applyPresetExtensions() {
  const defaults = createPreset() === "loca" ? extensionCatalogCache?.defaults || [] : [];
  for (const input of document.querySelectorAll("#extCreateDefault input:not(:disabled), #extCreateAdd input")) {
    input.checked = defaults.includes(input.value);
  }
  renderCreateTabsLine();
}

async function submitCreateLoca(event) {
  event?.preventDefault();
  // The same panel edits an existing loca's identity (identity.js).
  if (locaPanelMode === "identity") return submitLocaIdentity();
  const name = $("extCreateName").value.trim();
  if (!name) { $("extCreateError").textContent = "A loca needs a name."; $("extCreateName").focus(); return; }
  const extensions = [...document.querySelectorAll("#extCreateDefault input:checked:not(:disabled), #extCreateAdd input:checked")]
    .map(input => input.value);
  const group = $("extCreateGroupField").classList.contains("hidden") ? "" : $("extCreateGroup").value;
  $("extCreateSubmit").disabled = true;
  $("extCreateError").textContent = "";
  const gen = state.buildingGen;
  try {
    const body = { ...(group ? { name, extensions, group } : { name, extensions }), ...identityCreateFields() };
    const response = await fetch(`${serverBase()}/rooms`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      $("extCreateError").textContent = `Could not create loca: ${await response.text()}`;
      $("extCreateSubmit").disabled = false;
      return;
    }
  } catch (error) {
    $("extCreateSubmit").disabled = false;
    if (buildingStale(gen)) return;
    $("extCreateError").textContent = "Loca creation could not be confirmed. Refresh Your Locas before retrying.";
    return;
  }
  closeExtDialog("extCreate");
  $("newRoomName").value = "";
  // A loca opened on a floor is on that floor's card at once (Floors tab).
  // Read before joinRoom: entering a loca drops the profile until it is read
  // again, and a floor list drawn in that gap would close the Floors tab.
  if (group && typeof fetchGroups === "function") await fetchGroups();
  if (buildingStale(gen)) return;
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

async function changeExtension(id, add, openTasks = "") {
  const room = state.room;
  $("extManageError").textContent = "";
  hideBoardOffAsk();
  for (const button of document.querySelectorAll("#extManageList button")) button.disabled = true;
  const gen = state.buildingGen;
  try {
    const query = openTasks ? `?open_tasks=${encodeURIComponent(openTasks)}` : "";
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/extensions/${encodeURIComponent(id)}${query}`, {
      method: add ? "PUT" : "DELETE", headers: adminHeaders({}),
    });
    if (!response.ok) {
      const said = await response.text();
      // Turning the Loca Board off with unfinished tasks: board.js asks.
      if (!(response.status === 409 && !add && askBoardOff(said))) $("extManageError").textContent = said || `HTTP ${response.status}`;
    } else if (room === state.room) {
      const view = await response.json();
      state.locaExtensions = view;
      // The settings frame follows; adopt the new set now so the list does
      // not flicker back while it travels.
      state.settings = { ...(state.settings || {}), extensions: view.enabled };
      renderSettings();
    }
  } catch (error) {
    if (!buildingStale(gen)) $("extManageError").textContent = "Could not reach the server. Nothing was changed.";
  }
  if (room === state.room) renderExtensionManage();
}

$("extCreateForm").onsubmit = (e) => submitCreateLoca(e);
$("extCreateCancel").onclick = () => closeExtDialog("extCreate");
$("extCreateBackdrop").onclick = () => closeExtDialog("extCreate");
$("extCreate").addEventListener("change", (e) => {
  if (e.target.matches("input[type=checkbox]")) renderCreateTabsLine();
  if (e.target.id === "extCreateGroup") syncCreateGroupNote();
});
$("extManageClose").onclick = () => closeExtDialog("extManage");
$("extManageBackdrop").onclick = () => closeExtDialog("extManage");
$("extManageList").onclick = (e) => {
  const add = e.target.closest("[data-ext-add]");
  const remove = e.target.closest("[data-ext-remove]");
  if (add) changeExtension(add.dataset.extAdd, true);
  else if (remove) changeExtension(remove.dataset.extRemove, false);
};
$("locaSummary").addEventListener("click", (e) => {
  if (e.target.closest("#extManageOpen")) openManageExtensions();
  else if (e.target.closest("#extRetry")) fetchLocaExtensions();
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!$("extCreate").classList.contains("hidden")) closeExtDialog("extCreate");
  else if (!$("extManage").classList.contains("hidden")) closeExtDialog("extManage");
});
