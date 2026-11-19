"use strict";
// Two deliberately different sidebar perspectives: the Building and the
// currently selected Loca. Identity stays above both because it is the same
// principal in either context; authority and room state are rendered here.

let locaSidebarFetchSequence = 0;
let roomPreferenceKey = "";

function resetRoomPreferenceIdentity() {
  state.principalId = null;
  state.roomPreferences = { pinned: [], hidden: [], order: [], collapsed: [] };
  roomPreferenceKey = "";
}

function loadRoomPreferences() {
  const principal = state.principalId;
  if (!principal) {
    if (roomPreferenceKey) resetRoomPreferenceIdentity();
    return state.roomPreferences;
  }
  // The Host's own server takes a new port at every start, so its address
  // cannot name the place: there the place is "host" (one Host, one server).
  const place = window.__LOCA_LOCK_SERVER__ === true ? "host" : serverBase();
  const key = `loca-room-preferences:${place}:${principal}`;
  if (key === roomPreferenceKey) return state.roomPreferences;
  roomPreferenceKey = key;
  try {
    const parsed = JSON.parse(localStorage.getItem(key) || "{}");
    state.roomPreferences = {
      pinned: Array.isArray(parsed.pinned) ? parsed.pinned : [],
      hidden: Array.isArray(parsed.hidden) ? parsed.hidden : [],
      order: Array.isArray(parsed.order) ? parsed.order : [],
      // The floors this viewer closed in "Your Locas", by floor key.
      collapsed: Array.isArray(parsed.collapsed) ? parsed.collapsed : [],
    };
  } catch (error) {
    state.roomPreferences = { pinned: [], hidden: [], order: [], collapsed: [] };
  }
  return state.roomPreferences;
}

function saveRoomPreferences() {
  if (!roomPreferenceKey || !state.principalId) return;
  try { localStorage.setItem(roomPreferenceKey, JSON.stringify(state.roomPreferences)); }
  catch (error) { /* Personal navigation still works for this page lifetime. */ }
}

// Close or open a floor of "Your Locas" for this viewer. Kept with the rest
// of the sidebar's personal order, and so cleared with the identity; before
// the identity is known it lasts for this page only.
function setFloorCollapsed(key, collapsed) {
  const prefs = loadRoomPreferences();
  const others = prefs.collapsed.filter(value => value !== key);
  if (collapsed === prefs.collapsed.includes(key)) return;
  prefs.collapsed = collapsed ? [...others, key] : others;
  saveRoomPreferences();
}

// ---- the search at the head of the list --------------------------------
// A name filter over the locas and Messages, kept for this page only and
// applied each time the list is drawn (state.js renderRooms, dm.js
// renderDmList). Typing narrows the list at once; Escape clears it and the
// focus stays in the field.
let sidebarQuery = "";
let sidebarQueryShown = "";
function sidebarFilterActive() { return !!sidebarQuery; }
function sidebarQueryText() { return sidebarQueryShown; }
function sidebarMatches(...names) {
  if (!sidebarQuery) return true;
  return names.some(name => String(name || "").toLowerCase().includes(sidebarQuery));
}
function setSidebarQuery(value) {
  const shown = String(value || "").trim();
  const query = shown.toLowerCase();
  if (query === sidebarQuery && shown === sidebarQueryShown) return;
  sidebarQuery = query;
  sidebarQueryShown = shown;
  renderRooms();
}
$("sideSearch").addEventListener("input", event => setSidebarQuery(event.target.value));
$("sideSearch").addEventListener("keydown", event => {
  if (event.key !== "Escape" || !event.target.value) return;
  // Clear here; the page's own Escape (the phone's drawer, open panels)
  // waits for a second press.
  event.preventDefault();
  event.stopPropagation();
  event.target.value = "";
  setSidebarQuery("");
});

// ---- a floor's + ---------------------------------------------------------
// Opens the create-loca panel (extensions.js openCreateLoca) with the floor
// chosen. The Ground floor's is everyone's, like the new-loca bar; a named
// floor's only where its group may take a new loca from this person (the
// panel offers a floor only then).
const FLOOR_NEW_ICON = '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>';

function floorGroup(key) {
  return (state.groups || []).find(g => g.id === key || g.name === key) || null;
}

function floorAcceptsNewLoca(key) {
  if (key === GROUND_FLOOR) return true;
  if (!isAdmin()) return false;
  if (!state.groupsLoaded) return true;
  return !!floorGroup(key)?.can_create_loca;
}

function floorNewLocaButton(floor) {
  const plus = document.createElement("button");
  plus.type = "button";
  plus.className = "floornew";
  plus.dataset.floorNew = floor.key;
  plus.setAttribute("aria-label", `New loca on ${floor.name}`);
  plus.dataset.tip = "New loca";
  plus.dataset.tipDetail = `Open a loca on ${floor.name}.`;
  plus.innerHTML = FLOOR_NEW_ICON;
  // Inside the heading: the click opens the panel, not the floor.
  plus.addEventListener("click", event => {
    event.preventDefault();
    event.stopPropagation();
    openFloorNewLoca(floor.key);
  });
  return plus;
}

function openFloorNewLoca(key) {
  const group = key === GROUND_FLOOR ? "" : floorGroup(key)?.id || key;
  openCreateLoca("", group);
}

function orderedSidebarRooms(rooms, includeHidden = false) {
  const prefs = loadRoomPreferences();
  const hidden = new Set(prefs.hidden);
  const pinned = new Set(prefs.pinned);
  const rank = new Map(prefs.order.map((room, index) => [room, index]));
  const fallbackRank = new Map(rooms.map((room, index) => [room.room, index]));
  return rooms
    .filter(room => includeHidden ? hidden.has(room.room) : !hidden.has(room.room))
    .sort((a, b) => {
      const pinDelta = Number(pinned.has(b.room)) - Number(pinned.has(a.room));
      if (pinDelta) return pinDelta;
      const aRank = rank.has(a.room) ? rank.get(a.room) : Number.MAX_SAFE_INTEGER;
      const bRank = rank.has(b.room) ? rank.get(b.room) : Number.MAX_SAFE_INTEGER;
      return aRank - bRank || fallbackRank.get(a.room) - fallbackRank.get(b.room);
    });
}

function roomPreferenceButton(action, room, label, text) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `roompref ${action}`;
  button.dataset.roomPreference = action;
  button.dataset.room = room;
  button.setAttribute("aria-label", `${label} ${room}`);
  button.title = `${label} ${room}`;
  button.textContent = text;
  button.disabled = !state.principalId;
  return button;
}

function renderRoomPreferenceActions(room) {
  const prefs = loadRoomPreferences();
  const actions = document.createElement("details");
  actions.className = "roomprefs";
  const trigger = document.createElement("summary");
  trigger.className = "roompreftrigger";
  trigger.setAttribute("aria-label", `More options for ${room}`);
  trigger.title = `More options for ${room}`;
  trigger.textContent = "⋯";
  const menu = document.createElement("div");
  menu.className = "roomprefmenu";
  menu.setAttribute("role", "menu");
  const pinned = prefs.pinned.includes(room);
  menu.append(
    roomPreferenceButton("pin", room, pinned ? "Unpin" : "Pin", pinned ? "Unpin" : "Pin"),
    roomPreferenceButton("up", room, "Move up", "Move up"),
    roomPreferenceButton("down", room, "Move down", "Move down"),
    roomPreferenceButton("hide", room, "Hide", "Hide"),
  );
  actions.append(trigger, menu);
  return actions;
}

function updateRoomPreference(action, room) {
  if (!state.principalId) return;
  const prefs = loadRoomPreferences();
  if (action === "pin") {
    prefs.pinned = prefs.pinned.includes(room)
      ? prefs.pinned.filter(value => value !== room)
      : [...prefs.pinned, room];
  } else if (action === "hide") {
    if (!prefs.hidden.includes(room)) prefs.hidden.push(room);
  } else if (action === "show") {
    prefs.hidden = prefs.hidden.filter(value => value !== room);
  } else if (action === "up" || action === "down") {
    // Within its floor: the neighbour above or below is the one drawn there.
    const floor = roomFloorKey(state.rooms.find(candidate => candidate.room === room));
    const group = orderedSidebarRooms(state.rooms).filter(candidate =>
      prefs.pinned.includes(candidate.room) === prefs.pinned.includes(room)
      && roomFloorKey(candidate) === floor
    );
    const index = group.findIndex(candidate => candidate.room === room);
    const next = action === "up" ? index - 1 : index + 1;
    if (index >= 0 && next >= 0 && next < group.length) {
      [group[index], group[next]] = [group[next], group[index]];
      const groupNames = group.map(candidate => candidate.room);
      const groupSet = new Set(groupNames);
      prefs.order = [...prefs.order.filter(value => !groupSet.has(value)), ...groupNames];
    }
  }
  saveRoomPreferences();
  renderRooms();
}

function renderHiddenRooms() {
  const hidden = orderedSidebarRooms(state.rooms, true);
  $("hiddenLocas").classList.toggle("hidden", !hidden.length);
  $("hiddenLocaCount").textContent = hidden.length;
  $("hiddenRoomList").innerHTML = hidden.map(room =>
    `<div class="hiddenroom"><span title="${esc(locaIdNote(room.room))}"># ${esc(locaName(room.room))}</span>` +
      `<button type="button" data-room-preference="show" data-room="${esc(room.room)}" aria-label="Show ${esc(locaName(room.room))}">show</button></div>`
  ).join("");
}

function roomPreferenceClick(event) {
  const button = event.target.closest("[data-room-preference]");
  if (!button) return;
  event.preventDefault();
  event.stopPropagation();
  button.closest("details")?.removeAttribute("open");
  updateRoomPreference(button.dataset.roomPreference, button.dataset.room);
}

function resetLocaContext(room) {
  state.locaContext = {
    room,
    operatorReady: false,
    settingsReady: false,
    goalsReady: false,
    profileReady: false,
  };
}

function markLocaContextReady(source, room = state.room) {
  if (!state.locaContext || state.locaContext.room !== room || state.room !== room) return;
  state.locaContext[`${source}Ready`] = true;
}

function isLocaContextReady(source) {
  return !!state.locaContext
    && state.locaContext.room === state.room
    && !!state.locaContext[`${source}Ready`];
}

function setSidebarView(view, focus = false) {
  const selected = view === "loca" && !!state.room && !isDm(state.room) ? "loca" : "building";
  state.sidebarView = selected;
  const building = selected === "building";
  $("sideBuildingView").classList.toggle("hidden", !building);
  // Beside the chat (locainfo.js) This Loca is always shown.
  $("sideLocaView").classList.toggle("hidden", building && !document.body.classList.contains("locainfo-on"));
  for (const [button, active] of [
    [$("sideBuildingTab"), building],
    [$("sideLocaTab"), !building],
  ]) {
    button.setAttribute("aria-selected", String(active));
    button.tabIndex = active ? 0 : -1;
    button.classList.toggle("active", active);
  }
  if (focus) (building ? $("sideBuildingTab") : $("sideLocaTab")).focus();
  syncLocaInfoButton();
}

/// The header's Info button: shown in a loca, pressed while This Loca is in
/// view (always, beside the chat on a wide screen).
function syncLocaInfoButton() {
  const button = $("locaInfoBtn");
  if (!button) return;
  button.classList.toggle("hidden", !state.room || isDm(state.room));
  const beside = document.body.classList.contains("locainfo-on");
  button.setAttribute("aria-pressed", String(beside || state.sidebarView === "loca"));
}

function sidebarLifecycle() {
  if (!isLocaContextReady("settings")) return "Loading";
  const summary = state.rooms.find(room => room.room === state.room);
  return state.settings?.archived || summary?.archived ? "Closed" : "Open";
}

function renderLocaSidebar() {
  if (typeof placeLocaInfo === "function") placeLocaInfo();
  syncLocaInfoButton();
  // The Lobby room has no table: whoever is in it is here now.
  $("onlineLabel").textContent = isLobbyRoom(state.room) ? "HERE NOW" : "AT THE TABLE";
  const tab = $("sideLocaTab");
  // A private conversation is not a loca: it has no operator, lead, roles or
  // extensions to show, and its id is nobody's business on screen.
  tab.disabled = !state.room || isDm(state.room);
  if (!state.room || isDm(state.room)) {
    $("locaSummary").innerHTML = "";
    if (state.sidebarView === "loca") setSidebarView("building");
    return;
  }
  // The Lobby room is no loca: nobody operates or leads it and it carries
  // nothing but talk. Its panel says what it is, then who is here.
  if (isLobbyRoom(state.room)) {
    $("locaSummary").innerHTML =
      `<div class="locasummaryhead"><span class="locaglyph">⌂</span><b>Lobby</b></div>` +
      `<p class="lobbyroomabout">Everyone in the building lands here first. Say hello, ask who is around; work happens in locas.</p>`;
    return;
  }
  const assignment = isLocaContextReady("operator") ? state.locaOperator?.appointed : null;
  const operator = !isLocaContextReady("operator") ? "Loading…"
    : assignment?.display_name || "Not assigned";
  const operatorSource = assignment ? "appointed" : "";
  const lead = !isLocaContextReady("settings") ? "Loading…" : state.settings?.lead || "No lead";
  const roles = isLocaContextReady("profile") ? state.profile?.loca?.roles || [] : [];
  const myRole = !isLocaContextReady("profile") ? "Loading…" : roles.length
    ? locaRoleLabels(state.profile.loca).join(" · ")
    : "PARTICIPANT";
  const lifecycle = sidebarLifecycle();
  const mark = (typeof locaMarkHtml === "function" && locaMarkHtml(state.settings))
    || `<span class="locaglyph">#</span>`;
  // What the loca is for comes first, then who answers for it, then what it
  // carries. Each fact is one row: the name on the left, its act on the right.
  $("locaSummary").innerHTML =
    `<div class="locasummaryhead">${mark}${locaTitle(state.room)
      ? `<span class="locanamecol"><b>${esc(locaName(state.room))}</b><small class="locaid">id ${esc(state.room)}</small></span>`
      : `<b>${esc(state.room)}</b>`}` +
      `<span class="lifecycle ${lifecycle.toLowerCase()}">${esc(lifecycle)}</span></div>` +
    locaIdentityBlock() +
    `<dl class="locafacts">` +
      `<div><dt>Operator</dt><dd>${esc(operator)}${operatorSource ? `<small>${esc(operatorSource)}</small>` : ""}</dd></div>` +
      `<div><dt>Lead</dt><dd>${esc(lead)}</dd></div>` +
      `<div class="myrole"><dt>Your role</dt><dd>${esc(myRole)}</dd></div>` +
      (typeof locaGroupFact === "function" ? locaGroupFact() : "") +
    `</dl>` +
    (typeof locaGroupMoveHtml === "function" ? locaGroupMoveHtml() : "") +
    locaExtensionsBlock();
}

async function fetchLocaSidebar() {
  const room = state.room;
  const sequence = ++locaSidebarFetchSequence;
  if (!room) {
    state.locaOperator = null;
    renderLocaSidebar();
    return;
  }
  fetchLocaExtensions();
  if (isLobbyRoom(room)) {
    state.locaOperator = null;
    markLocaContextReady("operator", room);
    renderLocaSidebar();
    return;
  }
  try {
    const response = await fetch(
      `${serverBase()}/rooms/${encodeURIComponent(room)}/operators`,
      { headers: adminHeaders({}) },
    );
    const operator = response.ok ? await response.json() : null;
    if (sequence !== locaSidebarFetchSequence || room !== state.room) return;
    state.locaOperator = operator;
    refreshSenderTags();
  } catch (error) {
    if (sequence !== locaSidebarFetchSequence || room !== state.room) return;
    state.locaOperator = null;
  }
  markLocaContextReady("operator", room);
  renderLocaSidebar();
}

function sidebarTabKeydown(event) {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const wantLoca = event.key === "ArrowRight" || event.key === "End";
  setSidebarView(wantLoca && state.room ? "loca" : "building", true);
}

$("sideBuildingTab").onclick = () => setSidebarView("building");
$("sideLocaTab").onclick = () => setSidebarView("loca");
$("sideBuildingTab").onkeydown = sidebarTabKeydown;
$("sideLocaTab").onkeydown = sidebarTabKeydown;
$("roomList").onclick = roomPreferenceClick;
$("hiddenRoomList").onclick = roomPreferenceClick;
setSidebarView("building");

// The phone's own controls: the four places at the foot of the screen and
// the header's info button. Each opens what is already there; nothing new
// is drawn. The click stays here: a click reaching the document closes the
// account menu (profile.js) as it opens.
$("mobileNav")?.addEventListener("click", event => {
  const where = event.target.closest("[data-mobile-nav]")?.dataset.mobileNav;
  if (!where) return;
  event.stopPropagation();
  if (where === "notifications") { $("notifBell")?.click(); return; }
  setMobileSidebar(true);
  if (where === "me") {
    const menu = $("whoami").querySelector("details.profilemenu");
    if (menu) menu.open = true;
    return;
  }
  setSidebarView("building");
  if (where === "messages") $("dmSection")?.scrollIntoView({ block: "nearest" });
});
$("mobileInfoBtn")?.addEventListener("click", () => {
  setMobileSidebar(true);
  setSidebarView("loca", true);
});
// Info (LocaHeader board) toggles This Loca. On a wide screen it stands on
// the right already (locainfo.js): Info brings it to the eye and to the
// keyboard. Narrower, it is the sidebar's second tab: Info opens it, and
// again goes back to Your Locas.
$("locaInfoBtn")?.addEventListener("click", () => {
  if (document.body.classList.contains("locainfo-on")) {
    const panel = $("locaInfo");
    panel.scrollTop = 0;
    panel.scrollIntoView({ block: "nearest" });
    panel.classList.remove("infoflash");
    void panel.offsetWidth;
    panel.classList.add("infoflash");
    if (!panel.hasAttribute("tabindex")) panel.tabIndex = -1;
    panel.focus({ preventScroll: true });
    return;
  }
  const phone = window.matchMedia("(max-width: 760px)").matches;
  const shown = state.sidebarView === "loca" && (!phone || document.body.classList.contains("sidebar-open"));
  if (shown) {
    setSidebarView("building");
    if (phone) setMobileSidebar(false);
    return;
  }
  if (phone) setMobileSidebar(true);
  setSidebarView("loca", true);
});
