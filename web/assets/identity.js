"use strict";
// A loca's identity (PRINCIPLES "A loca's identity"): what it is for, its own
// number of seats, an icon and a colour from closed lists, and a few labels.
// One panel sets it: the create screen (`+`, extensions.js) shows these
// fields, and the same panel in its "Loca identity" mode edits them from
// This Loca, read-only for whoever cannot manage the loca. The server
// validates everything; this file draws and collects.

// The loca icons, in the server's order (protocol::LOCA_ICONS; a test holds
// the two to the same list). Plain 24×24 strokes in the app's style. They are
// not the icons a person wears.
const LOCA_ICON_PATHS = {
  book: '<path d="M5 4h12a2 2 0 0 1 2 2v14H7a2 2 0 0 1-2-2z"/><path d="M5 18a2 2 0 0 1 2-2h12"/>',
  code: '<path d="M8 7l-5 5 5 5M16 7l5 5-5 5"/>',
  bug: '<rect x="8" y="7" width="8" height="12" rx="4"/><path d="M12 7V5M9 4l1.5 2M15 4l-1.5 2M4 11h4M16 11h4M4 17h4M16 17h4"/>',
  rocket: '<path d="M12 3c3 2 5 5 5 9l-2 4H9l-2-4c0-4 2-7 5-9z"/><circle cx="12" cy="10" r="1.5"/><path d="M9 16l-2 4M15 16l2 4"/>',
  flask: '<path d="M9 3h6M10 3v6l-5 10a1 1 0 0 0 1 2h12a1 1 0 0 0 1-2l-5-10V3"/><path d="M7 15h10"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/>',
  chart: '<path d="M4 20V4M4 20h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/>',
  chat: '<path d="M4 5h16v11H9l-5 4z"/>',
  flag: '<path d="M5 21V4M5 4h12l-2 4 2 4H5"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
  heart: '<path d="M12 20s-7-4.5-7-10a4 4 0 0 1 7-2.5A4 4 0 0 1 19 10c0 5.5-7 10-7 10z"/>',
  house: '<path d="M4 11l8-7 8 7M6 10v10h12V10"/><path d="M10 20v-5h4v5"/>',
  bulb: '<path d="M9 18h6M10 21h4"/><path d="M12 3a6 6 0 0 0-3.5 10.9V16h7v-2.1A6 6 0 0 0 12 3z"/>',
  map: '<path d="M3 6l6-2 6 2 6-2v14l-6 2-6-2-6 2z"/><path d="M9 4v14M15 6v14"/>',
  music: '<path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/>',
  palette: '<path d="M12 3a9 9 0 1 0 0 18c1.5 0 2-1 2-2s-1-1.5-1-2.5 1-1.5 2-1.5h2a4 4 0 0 0 4-4c0-4.5-4-8-9-8z"/><circle cx="7.5" cy="11" r="1"/><circle cx="11" cy="7" r="1"/><circle cx="16" cy="8.5" r="1"/>',
  pen: '<path d="M4 20l1-5L16 4l4 4L9 19z"/><path d="M14 6l4 4"/>',
  puzzle: '<path d="M5 8h4a2 2 0 1 1 4 0h4v4a2 2 0 1 1 0 4v4H5z"/>',
  anchor: '<circle cx="12" cy="5" r="2"/><path d="M12 7v14M8 11h8M4 14a8 8 0 0 0 16 0"/>',
  compass: '<circle cx="12" cy="12" r="9"/><path d="M15.5 8.5l-2 5-5 2 2-5z"/>',
  camera: '<path d="M4 8h3l2-3h6l2 3h3v11H4z"/><circle cx="12" cy="13" r="3.5"/>',
  cloud: '<path d="M7 18h10a4 4 0 0 0 .5-8A6 6 0 0 0 6 9.5 4.3 4.3 0 0 0 7 18z"/>',
  cube: '<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z"/><path d="M4 7.5l8 4.5 8-4.5M12 12v9"/>',
  terminal: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M13 15h4"/>',
};

// The palette, in the server's order (protocol::LOCA_COLORS). The shades
// for each theme live in styles.css; the icon carries the meaning too.
const LOCA_COLOR_NAMES = {
  orange: "Orange",
  sky: "Sky",
  green: "Green",
  yellow: "Yellow",
  blue: "Blue",
  vermillion: "Vermillion",
  purple: "Purple",
};

const LOCA_TITLE_MAX = 64;
const LOCA_DESCRIPTION_MAX = 280;
const LOCA_SEATS_MIN = 2;

let locaPanelMode = "create";
// The Building's two numbers (`/health`): the seats of a loca without a
// number of its own, and the most a loca's own number may reach.
let locaSeatDefault = 7;
let locaSeatCeiling = 32;

// The create panel's presets. A preset fills fields the panel already has;
// it is not stored, not sent, and grants nothing: the loca it opens is an
// ordinary loca with exactly the values in the fields when Create is
// pressed, and the server decides every one of them. "loca" is the panel as
// it always was. "lounge" opens a place to talk: chat only (no extension
// ticked), every seat the Building allows, the chat icon. Who may enter is
// not a preset's business: the Floor choice stays where it is.
const LOCA_PRESETS = {
  loca: { seats: () => locaSeatDefault, icon: "", hint: "One short paragraph. Agents read it when they enter." },
  lounge: { seats: () => locaSeatCeiling, icon: "chat", hint: "What people talk about here. Agents read it when they enter." },
  mentat: { seats: () => locaSeatCeiling, icon: "book", hint: "Who should come here, and what to ask. Agents read it when they enter." },
};

function locaIconSvg(icon, size = 14) {
  const paths = LOCA_ICON_PATHS[icon];
  if (!paths) return "";
  return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true" fill="none" ` +
    `stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
}

/// The small square a loca wears beside its name: its icon (or `#`) in its
/// colour. Nothing at all for a loca without either, so it looks as before.
function locaMarkHtml(identity) {
  const icon = LOCA_ICON_PATHS[identity?.icon] ? identity.icon : "";
  const color = LOCA_COLOR_NAMES[identity?.color] ? identity.color : "";
  if (!icon && !color) return "";
  return `<span class="locamark"${color ? ` data-loca-color="${color}"` : ""} aria-hidden="true">` +
    `${icon ? locaIconSvg(icon) : "#"}</span>`;
}

/// A loca's title: what people see in place of its id ("" when it has none,
/// and the id is shown). Display only: every route, invite, command and
/// `@` keeps the id. The current loca's settings are the freshest; any other
/// loca's comes from the loca list.
function locaTitle(room) {
  if (!room || isDm(room)) return "";
  if (room === state.room && isLocaContextReady("settings")) return state.settings?.title || "";
  return (state.rooms || []).find(rm => rm.room === room)?.title || "";
}

/// What a person reads as a loca's name: its title, or its id.
function locaName(room) { return isLobbyRoom(room) ? "Lobby" : locaTitle(room) || room; }

/// Where a title is shown the id stays reachable: a tooltip line naming it.
function locaIdNote(room) { return locaTitle(room) ? `id: ${room}` : ""; }

/// The sidebar entry's tooltip: the name (and the id under a title), what it
/// is for, its labels.
function locaRowTitle(rm) {
  const lines = [locaName(rm.room)];
  if (locaIdNote(rm.room)) lines.push(locaIdNote(rm.room));
  if (rm.description) lines.push(rm.description);
  if (Array.isArray(rm.labels) && rm.labels.length) lines.push(`Labels: ${rm.labels.join(" · ")}`);
  if (rm.archived) lines.push("closed · read-only");
  return lines.join("\n");
}

/// Labels: a dashed, square-cornered tag in plain case. Never the rounded,
/// filled, upper-case shape of a rank or role badge: a label grants nothing.
function locaLabelsHtml(labels) {
  if (!Array.isArray(labels) || !labels.length) return "";
  return `<div class="localabels">${labels.map(label => `<span class="localabel">${esc(label)}</span>`).join("")}</div>`;
}

function locaSeatsOf(settings) {
  const own = Number(settings?.seats || 0);
  return own ? Math.min(own, locaSeatCeiling) : locaSeatDefault;
}

/// The Loca identity block under This Loca.
function locaIdentityBlock() {
  if (!state.room || isDm(state.room) || !isLocaContextReady("settings")) return "";
  const s = state.settings || {};
  const canManage = !!state.locaExtensions?.can_manage;
  const link = state.locaExtensions
    ? `<button type="button" class="extlink" id="identityOpen" aria-haspopup="dialog">${canManage ? "Edit" : "View"}</button>`
    : "";
  const description = s.description
    ? `<p class="locadesc">${esc(s.description)}</p>`
    : `<p class="locadesc empty">No description yet.</p>`;
  const seats = s.seats ? `<span class="locaseats">${esc(String(s.seats))} seats</span>` : "";
  return `<div class="locaident"><div class="locaexthead"><span>Loca identity</span>${link}</div>` +
    description + locaLabelsHtml(s.labels) + seats + `</div>`;
}

/// The loca header: the mark before the name and the description where
/// there is room. Called from renderSettings.
function applyLocaIdentity() {
  const s = !state.room || isDm(state.room) ? {} : state.settings || {};
  const icon = LOCA_ICON_PATHS[s.icon] ? s.icon : "";
  const color = LOCA_COLOR_NAMES[s.color] ? s.color : "";
  const holder = $("roomMark");
  holder.innerHTML = icon ? locaIconSvg(icon, 16) : color ? "#" : "";
  holder.classList.toggle("hidden", !icon && !color);
  if (color) holder.dataset.locaColor = color;
  else delete holder.dataset.locaColor;
  $("curRoom").classList.toggle("marked", !!(icon || color));
  // The title in the header (the Building view keeps its own word), the id
  // in its tooltip.
  if (state.room && !isDm(state.room) && !isBuildingView()) {
    $("curRoom").textContent = roomLabel(state.room);
  }
  $("curRoom").title = state.room && !isDm(state.room) && !isBuildingView()
    ? locaIdNote(state.room) : "";
  // A titled loca shows its id beside the title, as people type it.
  const id = $("roomId");
  if (id) {
    const titled = !!(state.room && !isDm(state.room) && !isBuildingView() && locaTitle(state.room));
    id.textContent = titled ? `#${state.room}` : "";
    id.classList.toggle("hidden", !titled);
  }
  const desc = $("roomDesc");
  desc.textContent = s.description || "";
  desc.title = s.description || "";
  desc.classList.toggle("hidden", !s.description);
}

/* ---- the panel ---- */

// Edit loca shows the first icons and a "+N" chip for the rest (EditLoca
// board). The create panel shows them all.
const LOCA_ICONS_SHOWN = 7;

function renderIdentityPickers() {
  const icons = Object.keys(LOCA_ICON_PATHS);
  const iconOpts = [`<label class="identopt" title="No icon"><input type="radio" name="locaIcon" value="">` +
      `<span class="identswatch none">#</span><span class="sr">No icon</span></label>`]
    .concat(icons.map((id, index) =>
      `<label class="identopt${index >= LOCA_ICONS_SHOWN ? " identmore" : ""}" title="${esc(id)}"><input type="radio" name="locaIcon" value="${esc(id)}">` +
      `<span class="identswatch">${locaIconSvg(id, 16)}</span><span class="sr">${esc(id)}</span></label>`));
  const more = icons.length - LOCA_ICONS_SHOWN;
  if (more > 0) {
    iconOpts.push(`<button type="button" class="identmorebtn" id="locaIconMore" aria-expanded="true" ` +
      `aria-label="More icons (${more})">+${more}</button>`);
  }
  $("locaIconOpts").innerHTML = iconOpts.join("");
  const colorOpts = [`<label class="identopt" title="No colour"><input type="radio" name="locaColor" value="">` +
      `<span class="identswatch none">—</span><span class="sr">No colour</span></label>`]
    .concat(Object.entries(LOCA_COLOR_NAMES).map(([id, name]) =>
      `<label class="identopt" title="${esc(name)}"><input type="radio" name="locaColor" value="${esc(id)}">` +
      `<span class="identswatch" data-loca-color="${esc(id)}"><span class="identdot"></span></span><span class="sr">${esc(name)}</span></label>`));
  $("locaColorOpts").innerHTML = colorOpts.join("");
}

/// Show only the first icons (and "+N"), or all of them. Never hides the
/// icon that is chosen.
function setIconPickerCollapsed(collapsed) {
  const picked = pickedValue("locaIcon");
  const pickedHidden = !!picked && Object.keys(LOCA_ICON_PATHS).indexOf(picked) >= LOCA_ICONS_SHOWN;
  const on = collapsed && !pickedHidden;
  $("locaIconPick").classList.toggle("collapsed", on);
  $("locaIconMore")?.setAttribute("aria-expanded", String(!on));
}

function pickedValue(name) {
  return document.querySelector(`#locaIdentityFields input[name=${name}]:checked`)?.value || "";
}

function setPicked(name, value) {
  const input = document.querySelector(`#locaIdentityFields input[name=${name}][value="${CSS.escape(value || "")}"]`)
    || document.querySelector(`#locaIdentityFields input[name=${name}][value=""]`);
  if (input) input.checked = true;
}

function syncDescCount() {
  $("locaDescCount").textContent = `${[...$("locaDesc").value].length} / ${LOCA_DESCRIPTION_MAX}`;
}

function syncTitleCount() {
  $("locaTitleCount").textContent = `${[...$("locaTitle").value].length} / ${LOCA_TITLE_MAX}`;
}

/// Fill the identity fields from `settings` (empty for a new loca).
function fillIdentityFields(settings) {
  $("locaTitle").value = settings?.title || "";
  syncTitleCount();
  $("locaDesc").value = settings?.description || "";
  $("locaSeats").min = String(LOCA_SEATS_MIN);
  $("locaSeats").max = String(locaSeatCeiling);
  $("locaSeats").value = String(locaSeatsOf(settings));
  delete $("locaSeats").dataset.touched;
  $("locaLabels").value = (settings?.labels || []).join(", ");
  setPicked("locaIcon", settings?.icon || "");
  setPicked("locaColor", settings?.color || "");
  syncDescCount();
}

/// The preset chosen on the create panel: "loca" unless Lounge is picked.
function createPreset() {
  const picked = document.querySelector("#locaPreset input:checked")?.value;
  return LOCA_PRESETS[picked] ? picked : "loca";
}

/// Fill the fields from the chosen preset. Everything stays editable after.
function applyCreatePreset() {
  const preset = LOCA_PRESETS[createPreset()];
  $("locaSeats").value = String(preset.seats());
  delete $("locaSeats").dataset.touched;
  setPicked("locaIcon", preset.icon);
  $("locaDesc").placeholder = preset.hint;
  // A mentat room is born with the Wiki and the Vault: it asks for a topic
  // and a mentat instead of extensions.
  const mentat = createPreset() === "mentat";
  $("mentatCreateFields").classList.toggle("hidden", !mentat);
  $("extCreateExtensions").classList.toggle("hidden", mentat);
  $("extCreateTabs").classList.toggle("hidden", mentat);
  $("extCreateSubmit").textContent = mentat ? "Create mentat room" : "Create loca";
  if (mentat && typeof loadMentatCandidates === "function") loadMentatCandidates($("mentatPick"), "");
  applyPresetExtensions();
}

/// Back to the panel as it always was: the "loca" preset, its hint.
function resetCreatePreset() {
  const loca = document.querySelector("#locaPreset input[value=loca]");
  if (loca) loca.checked = true;
  $("locaDesc").placeholder = LOCA_PRESETS.loca.hint;
  $("mentatCreateFields").classList.add("hidden");
  $("mentatTopic").value = "";
  $("extCreateSubmit").textContent = "Create loca";
}

function setIdentityFieldsDisabled(disabled) {
  for (const el of document.querySelectorAll("#locaIdentityFields input, #locaIdentityFields textarea")) {
    el.disabled = disabled;
  }
}

/// The Building's default and ceiling (`/health`), the seats control's
/// default and range. A server without the ceiling field offers the default
/// as the most.
async function refreshSeatCeiling() {
  try {
    const r = await fetch(`${serverBase()}/health`);
    if (r.ok) {
      const health = await r.json();
      const n = Number(health.loca_capacity);
      if (Number.isFinite(n) && n >= LOCA_SEATS_MIN) locaSeatDefault = n;
      const max = Number(health.loca_capacity_max);
      locaSeatCeiling = Number.isFinite(max) && max >= locaSeatDefault ? max : locaSeatDefault;
    }
  } catch (error) { /* keep the last numbers; the server decides anyway */ }
}

function identityLabelsDraft() {
  return $("locaLabels").value.split(",").map(label => label.trim()).filter(Boolean);
}

/// The fields as the panel holds them now.
function identityDraft() {
  return {
    title: $("locaTitle").value.replace(/[\r\n]+/g, " ").trim(),
    description: $("locaDesc").value.replace(/[\r\n]+/g, " ").trim(),
    seats: Number($("locaSeats").value),
    icon: pickedValue("locaIcon"),
    color: pickedValue("locaColor"),
    labels: identityLabelsDraft(),
  };
}

/// What a new loca is opened with: only what was given, so Enter straight
/// through sends the same body as before identity existed.
function identityCreateFields() {
  const draft = identityDraft();
  const body = {};
  if (draft.description) body.description = draft.description;
  if (Number.isFinite(draft.seats) && draft.seats && draft.seats !== locaSeatDefault) body.seats = draft.seats;
  if (draft.icon) body.icon = draft.icon;
  if (draft.color) body.color = draft.color;
  if (draft.labels.length) body.labels = draft.labels;
  return body;
}

/// The change from `settings`, field by field; empty when nothing changed.
function identityChangedFields(settings) {
  const draft = identityDraft();
  const body = {};
  if (draft.title !== (settings?.title || "")) body.title = draft.title;
  if (draft.description !== (settings?.description || "")) body.description = draft.description;
  if (Number.isFinite(draft.seats) && draft.seats && draft.seats !== locaSeatsOf(settings)) body.seats = draft.seats;
  if (draft.icon !== (settings?.icon || "")) body.icon = draft.icon;
  if (draft.color !== (settings?.color || "")) body.color = draft.color;
  if (draft.labels.join("\n") !== (settings?.labels || []).join("\n")) body.labels = draft.labels;
  return body;
}

/// Put the shared panel in "create" or "identity" mode.
function setLocaPanelMode(mode, canManage = true) {
  locaPanelMode = mode;
  const create = mode === "create";
  $("extCreateTitle").textContent = create ? "Open a new loca" : "Loca identity";
  $("extCreateName").disabled = !create;
  if (!create) $("extCreateNameHint").classList.add("hidden");
  $("locaPreset").classList.toggle("hidden", !create);
  if (!create) resetCreatePreset();
  // A new loca is opened under its name (its id); a title is given later.
  $("locaTitleField").classList.toggle("hidden", create);
  $("extCreateExtensions").classList.toggle("hidden", !create);
  $("extCreateTabs").classList.toggle("hidden", !create);
  if (!create) $("extCreateGroupField").classList.add("hidden");
  $("extCreateSubmit").textContent = create ? "Create loca" : "Save";
  $("extCreateSubmit").classList.toggle("hidden", !canManage);
  $("extCreateCancel").textContent = canManage ? "Cancel" : "Close";
  setIdentityFieldsDisabled(!canManage);
  const note = $("locaIdentityNote");
  note.textContent = create ? ""
    : canManage ? "Changes are written to the Journal with your name."
    : "Only this loca's operator, the Master or a Smaster changes its identity.";
  note.classList.toggle("hidden", create);
  if (create) setIconPickerCollapsed(false);
  renderIdentityPages();
}

/// Edit loca › Pages: the loca's pages in one line, and the extension
/// manager behind "Manage extensions" (View for whoever cannot change them).
function renderIdentityPages() {
  const strip = $("locaIdentityPages");
  if (!strip) return;
  const show = locaPanelMode === "identity" && !!state.room && !isDm(state.room) && !!state.locaExtensions;
  strip.classList.toggle("hidden", !show);
  if (!show) return;
  const pages = typeof locaPageNames === "function" ? locaPageNames() : null;
  $("locaIdentityPageList").textContent = !pages ? "Loading…" : pages.length ? pages.join(" · ") : "Chat only";
  $("identityManageExt").textContent = state.locaExtensions.can_manage ? "Manage extensions" : "View extensions";
}

/// Called by openCreateLoca: fresh fields for a new loca.
function resetIdentityForCreate() {
  setLocaPanelMode("create");
  fillIdentityFields(null);
  resetCreatePreset();
  refreshSeatCeiling().then(() => {
    if (locaPanelMode !== "create") return;
    // The default follows the Building's numbers (the chosen preset's: the
    // default or the ceiling); a number typed stays.
    $("locaSeats").max = String(locaSeatCeiling);
    if (!$("locaSeats").dataset.touched) $("locaSeats").value = String(LOCA_PRESETS[createPreset()].seats());
  });
}

/// The panel belongs to one person and, as "Loca identity", to one loca.
/// When that ends (sign-out, a session the server ended, another identity,
/// another loca) it is closed and emptied: a loca's description and labels
/// are for those who may enter it, and nothing read or typed there stays in
/// the page for whoever comes next. Called by clearBuildingData (every way
/// out of a session) and by joinRoom.
function closeLocaPanel() {
  $("extCreate").classList.add("hidden");
  $("extCreateName").value = "";
  $("extCreateError").textContent = "";
  $("extCreateSubmit").disabled = false;
  setLocaPanelMode("create");
  fillIdentityFields(null);
  resetCreatePreset();
}

/// Another loca was entered: a panel showing the previous one's identity
/// goes; a new loca being set up stays.
function closeLocaIdentityOnRoomChange() {
  if (locaPanelMode === "identity") closeLocaPanel();
}

/// This Loca › Loca identity: the same panel, for the current loca.
async function openLocaIdentity() {
  if (!state.room || isDm(state.room)) return;
  const room = state.room;
  const gen = state.buildingGen;
  await refreshSeatCeiling();
  // The person signed out, or went to another loca, while that was out:
  // this loca's identity is not theirs to be shown any more.
  if (buildingStale(gen) || room !== state.room) return;
  const canManage = !!state.locaExtensions?.can_manage;
  // The Name field shows the id, the loca's address; the title is its own field.
  $("extCreateName").value = state.room;
  $("extCreateError").textContent = "";
  $("extCreateSubmit").disabled = false;
  setLocaPanelMode("identity", canManage);
  fillIdentityFields(state.settings);
  setIconPickerCollapsed(true);
  openExtDialog("extCreate", canManage ? "locaDesc" : "extCreateCancel");
}

/// Save the identity of the current loca. A refused save keeps the draft
/// and shows the server's sentence.
async function submitLocaIdentity() {
  const room = state.room;
  const body = identityChangedFields(state.settings);
  if (!Object.keys(body).length) { closeExtDialog("extCreate"); return; }
  $("extCreateSubmit").disabled = true;
  $("extCreateError").textContent = "";
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/settings`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      $("extCreateError").textContent = `Not saved: ${await response.text() || `HTTP ${response.status}`}`;
      $("extCreateSubmit").disabled = false;
      return;
    }
    const settings = await response.json();
    if (room === state.room) {
      state.settings = settings;
      renderSettings();
    }
  } catch (error) {
    $("extCreateSubmit").disabled = false;
    if (!buildingStale(gen)) $("extCreateError").textContent = "Could not reach the server. Nothing was changed.";
    return;
  }
  closeExtDialog("extCreate");
  refreshRooms();
}

renderIdentityPickers();
$("locaTitle").addEventListener("input", syncTitleCount);
$("locaDesc").addEventListener("input", () => {
  // One paragraph: a pasted line break becomes a space.
  const value = $("locaDesc").value;
  if (/[\r\n]/.test(value)) $("locaDesc").value = value.replace(/[\r\n]+/g, " ");
  syncDescCount();
});
// Enter in the description submits, as in every other field of the panel.
$("locaDesc").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    $("extCreateForm").requestSubmit();
  }
});
$("locaSeats").addEventListener("input", () => { $("locaSeats").dataset.touched = "1"; });
// "+N": the rest of the icons, and focus on the first of them.
$("locaIconOpts").addEventListener("click", (e) => {
  if (!e.target.closest("#locaIconMore")) return;
  setIconPickerCollapsed(false);
  $("locaIconOpts").querySelector(".identmore input")?.focus();
});
// An arrow key may walk the radio group onto an icon not shown yet.
$("locaIconOpts").addEventListener("change", (e) => {
  if (e.target.closest(".identmore")) setIconPickerCollapsed(false);
});
$("extCreateX").addEventListener("click", () => closeExtDialog("extCreate"));
$("identityManageExt").addEventListener("click", () => openManageExtensions());
$("locaPreset").addEventListener("change", applyCreatePreset);
$("locaSummary").addEventListener("click", (e) => { if (e.target.closest("#identityOpen")) openLocaIdentity(); });
