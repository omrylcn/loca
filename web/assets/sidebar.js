"use strict";
// Two deliberately different sidebar perspectives: the Building and the
// currently selected Loca. Identity stays above both because it is the same
// principal in either context; authority and room state are rendered here.

let locaSidebarFetchSequence = 0;
let roomPreferenceKey = "";

function resetRoomPreferenceIdentity() {
  state.principalId = null;
  state.roomPreferences = { pinned: [], hidden: [], order: [] };
  roomPreferenceKey = "";
}

function loadRoomPreferences() {
  const principal = state.principalId;
  if (!principal) {
    if (roomPreferenceKey) resetRoomPreferenceIdentity();
    return state.roomPreferences;
  }
  const key = `loca-room-preferences:${serverBase()}:${principal}`;
  if (key === roomPreferenceKey) return state.roomPreferences;
  roomPreferenceKey = key;
  try {
    const parsed = JSON.parse(localStorage.getItem(key) || "{}");
    state.roomPreferences = {
      pinned: Array.isArray(parsed.pinned) ? parsed.pinned : [],
      hidden: Array.isArray(parsed.hidden) ? parsed.hidden : [],
      order: Array.isArray(parsed.order) ? parsed.order : [],
    };
  } catch (error) {
    state.roomPreferences = { pinned: [], hidden: [], order: [] };
  }
  return state.roomPreferences;
}

function saveRoomPreferences() {
  if (!roomPreferenceKey || !state.principalId) return;
  try { localStorage.setItem(roomPreferenceKey, JSON.stringify(state.roomPreferences)); }
  catch (error) { /* Personal navigation still works for this page lifetime. */ }
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
    const group = orderedSidebarRooms(state.rooms).filter(candidate =>
      prefs.pinned.includes(candidate.room) === prefs.pinned.includes(room)
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
    `<div class="hiddenroom"><span># ${esc(room.room)}</span>` +
      `<button type="button" data-room-preference="show" data-room="${esc(room.room)}" aria-label="Show ${esc(room.room)}">show</button></div>`
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
  const selected = view === "loca" && !!state.room ? "loca" : "building";
  state.sidebarView = selected;
  const building = selected === "building";
  $("sideBuildingView").classList.toggle("hidden", !building);
  $("sideLocaView").classList.toggle("hidden", building);
  for (const [button, active] of [
    [$("sideBuildingTab"), building],
    [$("sideLocaTab"), !building],
  ]) {
    button.setAttribute("aria-selected", String(active));
    button.tabIndex = active ? 0 : -1;
    button.classList.toggle("active", active);
  }
  if (focus) (building ? $("sideBuildingTab") : $("sideLocaTab")).focus();
}

function sidebarLifecycle() {
  if (!isLocaContextReady("settings")) return "Loading";
  const summary = state.rooms.find(room => room.room === state.room);
  return state.settings?.archived || summary?.archived ? "Closed" : "Open";
}

function renderLocaSidebar() {
  const tab = $("sideLocaTab");
  tab.disabled = !state.room;
  if (!state.room) {
    $("locaSummary").innerHTML = "";
    if (state.sidebarView === "loca") setSidebarView("building");
    return;
  }
  const assignment = isLocaContextReady("operator") ? state.locaOperator?.appointed : null;
  const inherited = isLocaContextReady("operator") ? state.locaOperator?.inherited_master : null;
  const operator = !isLocaContextReady("operator") ? "Loading…"
    : assignment?.display_name || inherited?.display_name || "Not assigned";
  const operatorSource = assignment ? "appointed" : inherited ? "inherited from Master" : "";
  const lead = !isLocaContextReady("settings") ? "Loading…" : state.settings?.lead || "No lead";
  const roles = isLocaContextReady("profile") ? state.profile?.loca?.roles || [] : [];
  const myRole = !isLocaContextReady("profile") ? "Loading…" : roles.length
    ? roles.filter(role => role !== "participant" || roles.length === 1).map(profileRoleLabel).join(" · ")
    : "PARTICIPANT";
  const lifecycle = sidebarLifecycle();
  $("locaSummary").innerHTML =
    `<div class="locasummaryhead"><span class="locaglyph">#</span><span><b>${esc(state.room)}</b>` +
      `<small>This Loca · ${esc(lifecycle)}</small></span><span class="lifecycle ${lifecycle.toLowerCase()}">${esc(lifecycle)}</span></div>` +
    `<dl class="locafacts">` +
      `<div><dt>Operator</dt><dd>${esc(operator)}${operatorSource ? `<small>${esc(operatorSource)}</small>` : ""}</dd></div>` +
      `<div><dt>Lead</dt><dd>${esc(lead)}</dd></div>` +
      `<div><dt>Your role</dt><dd>${esc(myRole)}</dd></div>` +
    `</dl>`;
}

async function fetchLocaSidebar() {
  const room = state.room;
  const sequence = ++locaSidebarFetchSequence;
  if (!room) {
    state.locaOperator = null;
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
