"use strict";
// Server-derived identity, composed Building/Loca roles, and personal access.

function profileRoleLabel(role) {
  return String(role || "member").replaceAll("_", " ").toUpperCase();
}

// One's roles in a loca, as shown. Operator is a seat a person is appointed
// to; the Master holds its authority in every loca by rank, not by
// appointment (PRINCIPLES): that is shown as the rank, never as OPERATOR.
function locaRoleLabels(loca) {
  const roles = loca?.roles || [];
  return roles
    .filter(role => role !== "participant" || roles.length === 1)
    .map(role => role === "operator" && loca?.operator_source === "master" ? "MASTER" : profileRoleLabel(role));
}

function profileKindLabel(kind) {
  return kind === "agent" ? "Agent" : "Human";
}

function profileTime(value) {
  if (!value) return "no expiry";
  return new Date(Number(value)).toLocaleString([], {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  });
}

const APPEARANCE_LABELS = {
  theme: { dark: "Dark", light: "Light" },
  accent: { teal: "Teal", blue: "Blue", violet: "Violet", amber: "Amber", rose: "Rose" },
  density: { compact: "Compact", comfortable: "Comfortable", spacious: "Spacious" },
  font_size: { small: "Small", normal: "Normal", large: "Large", larger: "Larger" },
  font_family: { mono: "Mono", sans: "Sans", serif: "Serif" },
};
const APPEARANCE_ROWS = [
  ["theme", "Theme"], ["accent", "Accent"], ["font_family", "Font"], ["font_size", "Text"], ["density", "Density"],
];

function currentAppearance() {
  return state.appearance || cachedAppearance();
}

// Personal look: real radio groups, applied instantly, saved per principal.
function profileAppearanceHtml() {
  const prefs = currentAppearance();
  const rows = APPEARANCE_ROWS.map(([key, title]) => {
    const labelId = `appearanceLabel-${key}`;
    const options = APPEARANCE_CHOICES[key].map(value => {
      const name = APPEARANCE_LABELS[key][value];
      const checked = prefs[key] === value ? " checked" : "";
      // A font choice is drawn in its own face, so the chip is its preview.
      const face = key === "accent"
        ? `<span class="swatch ${value}" title="${name}" aria-hidden="true"></span><span class="visuallyhidden">${name}</span>`
        : key === "font_family" ? `<span class="fontface ${value}">${name}</span>` : `<span>${name}</span>`;
      return `<label class="appearanceopt"><input type="radio" name="appearance-${key}"` +
        ` data-appearance="${key}" value="${value}"${checked} />${face}</label>`;
    }).join("");
    return `<div class="appearancerow" role="radiogroup" aria-labelledby="${labelId}">` +
      `<span id="${labelId}">${title}</span><div class="appearanceopts">${options}</div></div>`;
  }).join("");
  // The sample is drawn with the chat's own rules (font, text size, bubble
  // padding), so each click previews here what it changes in the loca.
  const sample = `<div class="appearancepreview" id="appearancePreview" aria-hidden="true">` +
    `<div class="bubble"><div class="line"><span class="sender">${esc(state.name || "you")}</span></div>` +
    `<div class="body">The table is set. 0O 1lI</div></div></div>`;
  return `<div class="profileappearance" id="profileAppearance"><small>Appearance</small>${rows}${sample}` +
    `<div class="appearancestatus" id="appearanceStatus" role="status"></div></div>`;
}

function syncAppearanceInputs() {
  const prefs = currentAppearance();
  document.querySelectorAll("input[data-appearance]").forEach(input => {
    input.checked = prefs[input.dataset.appearance] === input.value;
  });
}

function setAppearanceStatus(text) {
  const status = $("appearanceStatus");
  if (status) status.textContent = text;
}

// Server copy wins over the local cache unless the person changed something
// while the read was in flight: their newer click is already being saved.
let appearanceEdits = 0;
async function fetchAppearance() {
  const principal = state.principalId;
  if (!principal) return;
  const editsAtStart = appearanceEdits;
  try {
    const response = await fetch(`${serverBase()}/profile/preferences`, { headers: adminHeaders({}) });
    if (!response.ok) return;
    const prefs = await response.json();
    if (editsAtStart !== appearanceEdits || principal !== state.principalId) return;
    state.appearance = applyAppearance(prefs);
    state.appearancePrincipal = principal;
    cacheAppearance(state.appearance);
    syncAppearanceInputs();
  } catch (error) {}
}

let appearanceSaveSequence = 0;
async function saveAppearance(key, value) {
  appearanceEdits++;
  state.appearance = applyAppearance({ ...currentAppearance(), [key]: value });
  cacheAppearance(state.appearance);
  const sequence = ++appearanceSaveSequence;
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/profile/preferences`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ [key]: value }),
    });
    if (!response.ok) throw new Error(await response.text());
    const saved = await response.json();
    if (sequence !== appearanceSaveSequence) return;
    state.appearance = applyAppearance(saved);
    cacheAppearance(state.appearance);
    setAppearanceStatus("");
  } catch (error) {
    if (sequence === appearanceSaveSequence && !buildingStale(gen)) setAppearanceStatus("Not saved to your profile; kept on this device.");
  }
}

// The account menu's own part: the server this page is signed in to, and the
// one way out, Sign out (app.js signOutHere). Shown whatever else the profile
// could or could not say, so leaving never depends on the profile loading.
function profileServerHtml() {
  return `<div class="profilesession profileserver"><small>Server</small><span id="accountServer">${esc(state.server || serverBase())}</span></div>`;
}
function profileSignOutHtml() {
  const master = state.profile?.building_role === "master";
  return `<div class="profilesignout"><button type="button" class="leavebtn" id="leaveBtn">Sign out</button>` +
    `<div class="leaveconfirm hidden" id="leaveConfirm" role="alertdialog" aria-label="Sign out of this browser">` +
      `Sign out of this browser? Your loca sessions here close. Your membership and your key stay; ` +
      (master ? "a fresh pairing code signs you in again." : "your key signs you in again.") +
      `<div class="choices"><button type="button" class="no" id="leaveNo">Cancel</button>` +
      `<button type="button" class="yes" id="leaveYes">Sign out</button></div></div></div>`;
}

// The account menu is redrawn whenever the profile is read again (a loca
// switch, a lead change, every journal entry while the Building view is open).
// A redraw for the same person keeps it as they had it: open, and with the
// Sign out confirmation still asked, so a click aimed at it is not lost to a
// menu that shut under the pointer. A new person (generation) starts closed.
function accountMenuState(box) {
  if (box.dataset.gen !== String(state.buildingGen)) return { open: false, confirming: false };
  const menu = box.querySelector("details.profilemenu");
  return {
    open: !!menu?.open,
    confirming: !!menu?.open && !!box.querySelector("#leaveConfirm:not(.hidden)"),
  };
}
function restoreAccountMenu(box, kept) {
  box.dataset.gen = String(state.buildingGen);
  if (!kept.open) return;
  const menu = box.querySelector("details.profilemenu");
  if (menu) menu.open = true;
  if (kept.confirming) $("leaveConfirm")?.classList.remove("hidden");
}

// While the person has the account menu open it is theirs: a name half
// typed, an icon picked but not saved, the one-time view of a new key, an
// opened section, the caret. A redraw that only follows the server (a
// journal entry, a lead change, a profile read again) would wipe all of it,
// so it waits until the menu closes. What the person did themselves (saved,
// renamed, made or revoked a key) redraws at once: those paths pass `now`.
// A new person (generation) is always drawn.
let profileRedrawWaiting = false;
function accountMenuInUse(box) {
  return box.dataset.gen === String(state.buildingGen)
    && !!box.querySelector("details.profilemenu")?.open;
}
function profileRedrawMustWait(box, now) {
  if (now || !accountMenuInUse(box)) {
    profileRedrawWaiting = false;
    return false;
  }
  profileRedrawWaiting = true;
  return true;
}
// `toggle` does not bubble: caught on the way down.
$("whoami").addEventListener("toggle", event => {
  if (!event.target.matches?.("details.profilemenu") || event.target.open) return;
  if (profileRedrawWaiting) renderProfile();
}, true);

function renderProfile(now = false) {
  const box = $("whoami");
  if (profileRedrawMustWait(box, now)) return;
  const profile = state.profile;
  const kept = accountMenuState(box);
  if (!profile?.principal) {
    box.innerHTML = `<details class="profilemenu"><summary>` +
      `<span class="profileidentity"><span class="profileglyph">.</span>` +
      `<span><b>${esc(state.name)}</b><small>connected as ${esc(state.name)}</small></span></span>` +
      `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
      `<div class="profilepanel"><div class="profileempty">Profile is available after you take a seat.</div>${profileServerHtml()}${profileSignOutHtml()}</div></details>`;
    restoreAccountMenu(box, kept);
    box.classList.add("on");
    return;
  }
  const principal = profile.principal;
  const loca = profile.loca;
  const locaLabels = locaRoleLabels(loca);
  const source = loca?.operator_source === "appointed" ? "appointed" : "";
  const management = loca?.building_management ? "Building management access" : "";
  const locaText = locaLabels.length ? locaLabels.join(" · ") : "PARTICIPANT";
  const expiry = profile.session?.expires_at
    ? `expires ${profileTime(profile.session.expires_at)}` : "verified credential";
  const credentials = (state.credentials || []).map(credential => {
    const stateText = credential.revoked_at ? "revoked"
      : credential.current ? "current" : "active";
    const last = credential.last_used_at ? `used ${profileTime(credential.last_used_at)}` : "not used yet";
    const action = credential.root_recovery || credential.revoked_at ? ""
      : `<button type="button" data-revoke-credential="${esc(credential.id)}"` +
        ` data-current="${credential.current ? "1" : "0"}">` +
        `${credential.current ? "revoke & sign out" : "revoke"}</button>`;
    return `<div class="credentialrow ${credential.revoked_at ? "revoked" : ""}">` +
      `<span><b>${esc(credential.label)}</b><small>${esc(last)}</small></span>` +
      `<span class="credentialstate">${esc(stateText)}${credential.root_recovery ? " · recovery" : ""}</span>${action}</div>`;
  }).join("");
  box.innerHTML = `<details class="profilemenu"><summary aria-label="Open your account menu">` +
    `<span class="profileidentity"><span class="profileglyph">${principal.kind === "agent" ? "*" : "."}</span>` +
      `<span><b>${personLabelHtml(principal.display_name, cardOf(principal))}</b><small>You · ${profileKindLabel(principal.kind)}</small></span></span>` +
    `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
    `<div class="profilepanel"><div class="profilepanelhead"><span>Profile</span><small>Identity &amp; access</small></div>` +
    `<div class="profileconnected">connected as <b>${esc(principal.display_name)}</b>${rankBadge(profile.building_role)} · You · ${profileKindLabel(principal.kind)}</div>` +
    profileServerHtml() +
    profileRenameHtml(profile, principal) +
    profileCardHtml(principal) +
    `<div class="profileroles">` +
      `<span><small>Building</small><b class="role building-${esc(profile.building_role)}">${profileRoleLabel(profile.building_role)}</b></span>` +
      `<span><small>This Loca</small><b class="role loca-role">${esc(locaText)}</b>${source ? `<em>${source}</em>` : ""}${management ? `<em>${management}</em>` : ""}</span>` +
    `</div>` +
    profileGroupsHtml(profile) +
    `<div class="profilesession"><small>Session</small><span>${profile.session ? "Bounded session" : "Direct credential"} · ${esc(expiry)}</span></div>` +
    profileAppearanceHtml() +
    `<details class="profileaccess"><summary>Your access <span>${state.credentials.filter(c => !c.revoked_at).length}</span></summary>` +
      `<div class="credentiallist">${credentials || '<div class="credentialnone">No access keys listed.</div>'}</div>` +
      `<div class="credentialcreate"><input id="credentialLabel" maxlength="64" placeholder="New key label, e.g. MacBook" />` +
      `<button type="button" id="credentialCreate">Create key</button></div>` +
      `<div class="credentialsecret hidden" id="credentialSecret" role="status"></div>` +
    `</details>${profileSignOutHtml()}</div></details>`;
  restoreAccountMenu(box, kept);
  box.classList.add("on");
}

// The groups I belong to. Everyone is in the default group; a group adds
// the locas only its people see.
function profileGroupsHtml(profile) {
  const groups = Array.isArray(profile.groups) ? profile.groups : [];
  const chips = groups.length
    ? groups.map(g => `<span class="grpchip">${esc(g.name)}</span>`).join("")
    : `<span class="pnowhere">Ground floor only</span>`;
  return `<div class="profilegroups"><small>Floors</small><span class="grpchips">${chips}</span></div>`;
}

// ---- A person's card: who they are and their groups ----------------------
// Opened from a name in the Building view. The founder of a group (or the
// Master) adds and removes the person here; everyone else only reads. Built
// from the groups the viewer may see, so a card never reveals a hidden one.
let personCardName = null;
let personCardReturnFocus = null;

function personCardProfile(name) {
  return (state.profilesList || []).find(p => p.display_name === name) || null;
}

function renderPersonCard() {
  const name = personCardName;
  if (!name) return;
  const person = personCardProfile(name);
  const resident = (state.people || []).find(p => p.name === name);
  const kind = person?.kind === "agent" || resident?.kind === "agent" ? "agent" : "human";
  const role = person?.role || "member";
  $("personCardTitle").innerHTML = `${personLabelHtml(name, cardOf(person) || cardOf(resident))}${rankBadge(role)}<small>${kind === "agent" ? "Agent" : "Human"}</small>`;
  const theirs = groupsOfPerson(name);
  const chips = theirs.length ? theirs.map(g => {
    const founder = !g.owned_by_master && person && g.owner_id === person.id;
    const remove = g.can_manage && !founder && person
      ? `<button type="button" class="btn danger grpremove" data-card-remove="${esc(g.id)}" aria-label="Remove ${esc(name)} from ${esc(g.name)}" title="Remove from ${esc(g.name)}">${GROUP_REMOVE_ICON}<span>Remove</span></button>`
      : "";
    return `<div class="grpmember"><span class="pname">${GROUP_ICON}<b>${esc(g.name)}</b>${founder ? `<span class="pkind">founder</span>` : ""}</span><span class="pacts">${remove}</span></div>`;
  }).join("") : `<div class="bbempty">Ground floor only.</div>`;
  let add = "";
  if (role === "master") {
    add = `<p class="extnote">The Master sees every floor and is never a member of one.</p>`;
  } else if (person && person.id !== state.principalId) {
    const open = (state.groups || []).filter(g =>
      g.can_manage && !g.archived && !(g.members || []).some(m => m.principal_id === person.id));
    if (open.length) {
      add = `<div class="grpadd"><select id="personCardGroup" aria-label="Floor to add ${esc(name)} to">` +
        open.map(g => `<option value="${esc(g.id)}">${esc(g.name)}</option>`).join("") +
        `</select><button type="button" class="btn primary" id="personCardAdd">Add to floor</button></div>`;
    }
  }
  $("personCardBody").innerHTML =
    `<div class="extsectionhead">Floors</div><div class="grpmembers">${chips}</div>${add}`;
}

async function openPersonCard(name) {
  personCardName = name;
  personCardReturnFocus = document.activeElement;
  $("personCardError").textContent = "";
  const gen = state.buildingGen;
  if (!state.groupsLoaded || !state.profilesList) await fetchGroups(false);
  if (buildingStale(gen)) return;
  renderPersonCard();
  $("personCard").classList.remove("hidden");
  setTimeout(() => $("personCardClose")?.focus(), 0);
}

function closePersonCard() {
  $("personCard").classList.add("hidden");
  personCardName = null;
  if (personCardReturnFocus && document.contains(personCardReturnFocus)) personCardReturnFocus.focus();
  personCardReturnFocus = null;
}

async function personCardClick(event) {
  const person = personCardProfile(personCardName);
  const add = event.target.closest("#personCardAdd");
  const remove = event.target.closest("[data-card-remove]");
  if (!person || (!add && !remove)) return;
  const gen = state.buildingGen;
  const error = add
    ? await addToGroup($("personCardGroup").value, person.id)
    : await removeFromGroup(remove.dataset.cardRemove, person.id);
  if (buildingStale(gen)) return;
  $("personCardError").textContent = error;
  renderPersonCard();
}

$("personCardBody").addEventListener("click", (event) => personCardClick(event));
$("personCardClose").onclick = () => closePersonCard();
$("personCardBackdrop").onclick = () => closePersonCard();
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("personCard").classList.contains("hidden")) closePersonCard();
});

// Titles are not names. A Master or Smaster sets its own display name here;
// the server keeps identity, keys and rank, and refuses titles and names
// someone else already wears.
const PROFILE_TITLE_WORDS = ["master", "smaster", "operator", "lead", "admin", "root"];
function profileRenameHtml(profile, principal) {
  const role = profile.building_role;
  if (role !== "master" && role !== "smaster") return "";
  const isTitle = PROFILE_TITLE_WORDS.includes(String(principal.display_name).toLowerCase());
  const hint = isTitle
    ? `<div class="profilerenamehint">Your name is currently a title. Set your own name so people can tell who you are.</div>`
    : "";
  return `<div class="profilerename">${hint}` +
    `<label for="profileNameInput">Display name</label>` +
    `<div class="profilerenamerow"><input id="profileNameInput" maxlength="64" autocomplete="off" spellcheck="false" value="${esc(principal.display_name)}" />` +
    `<button type="button" id="profileRename">Rename</button></div>` +
    `<div class="profilerenamenote" id="profileRenameNote" role="status"></div></div>`;
}

// One's own full name and icon (PUT /profile/card). Anyone sets their own;
// the @name, and everything it decides, stays as it is.
function profileCardHtml(principal) {
  const card = cardOf(principal) || { full_name: "", icon: "" };
  const icons = ["", ...Object.keys(PROFILE_ICONS)].map(icon => {
    const label = icon ? icon[0].toUpperCase() + icon.slice(1) : "No icon";
    const face = icon
      ? `${profileIconSvg(icon)}<span class="visuallyhidden">${label}</span>`
      : "None";
    return `<label class="appearanceopt iconopt" title="${label}"><input type="radio" name="profile-icon"` +
      ` data-profile-icon value="${icon}"${card.icon === icon ? " checked" : ""} /><span>${face}</span></label>`;
  }).join("");
  return `<div class="profilerename profilecard" id="profileCard">` +
    `<label for="profileFullName">Full name</label>` +
    `<div class="profilerenamerow"><input id="profileFullName" maxlength="64" autocomplete="name" spellcheck="false"` +
    ` placeholder="e.g. Ada Lovelace" value="${esc(card.full_name)}" />` +
    `<button type="button" id="profileCardSave">Save</button></div>` +
    `<div class="appearanceopts profileicons" role="radiogroup" aria-label="Profile icon">${icons}</div>` +
    `<div class="profilecardhint">Shown beside @${esc(principal.display_name)}. People still call you @${esc(principal.display_name)}.</div>` +
    `<div class="profilerenamenote" id="profileCardNote" role="status"></div></div>`;
}

async function saveProfileCard() {
  const input = $("profileFullName");
  const note = $("profileCardNote");
  if (!input) return;
  const icon = $("whoami").querySelector("input[data-profile-icon]:checked")?.value || "";
  const gen = state.buildingGen;
  let response;
  try {
    response = await fetch(`${serverBase()}/profile/card`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ full_name: input.value, icon }),
    });
  } catch (e) {
    if (note && !buildingStale(gen)) note.textContent = "could not reach the server — not saved";
    return;
  }
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen) && $("profileCardNote")) $("profileCardNote").textContent = why;
    return;
  }
  const saved = await response.json();
  const principal = state.profile?.principal;
  if (buildingStale(gen) || !principal) return;
  principal.full_name = saved.full_name || undefined;
  principal.icon = saved.icon || undefined;
  renderProfile(true);
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const after = $("profileCardNote");
  if (after) after.textContent = "Saved.";
}

// Set the display name chosen at sign-in. Returns "" on success, or the
// server's reason when it is refused (a title, or a name someone else wears).
async function applyChosenName(name) {
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/profile`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ display_name: name }),
    });
    if (!response.ok) return (await response.text()) || "the name was refused";
    const own = (await response.json()).display_name;
    // The name is its asker's: never worn by whoever signed in meanwhile.
    if (!buildingStale(gen)) adoptOwnName(own);
    return "";
  } catch (e) {
    return "could not reach the server to set the name";
  }
}

function showRenameRefusal(name, reason) {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const note = $("profileRenameNote");
  if (note) note.textContent = `"${name}" was not set: ${reason}`;
  const input = $("profileNameInput");
  if (input) { input.value = name; input.focus(); }
}

async function renameProfile() {
  const input = $("profileNameInput");
  const note = $("profileRenameNote");
  const name = input?.value.trim();
  if (!name || name === state.profile?.principal?.display_name) return;
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile`, {
    method: "PUT",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ display_name: name }),
  });
  if (buildingStale(gen)) return;   // signed out meanwhile: not the next person's name
  if (!response.ok) {
    const why = await response.text();
    if (note && !buildingStale(gen)) note.textContent = why;
    return;
  }
  const renamed = await response.json();
  if (buildingStale(gen)) return;
  const before = state.name;
  adoptOwnName(renamed.display_name);
  // The server relabels history and tells every open feed; reconnecting
  // here covers a loca with nothing said yet (no frame arrives).
  if (before !== renamed.display_name && state.room) openWs();
  await fetchProfile(true);
  if (buildingStale(gen)) return;
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const after = $("profileRenameNote");
  if (after) after.textContent = `Renamed to ${renamed.display_name}.`;
}

let profileFetchSequence = 0;
async function fetchProfile(now = false) {
  const box = $("whoami");
  const room = state.room;
  const sequence = ++profileFetchSequence;
  const gen = state.buildingGen;
  try {
    const suffix = room ? `?room=${encodeURIComponent(room)}` : "";
    const [profileResponse, credentialResponse] = await Promise.all([
      fetch(`${serverBase()}/profile${suffix}`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/profile/credentials`, { headers: adminHeaders({}) }),
    ]);
    if (!profileResponse.ok) throw new Error("profile unavailable");
    const profile = await profileResponse.json();
    const credentials = credentialResponse.ok ? await credentialResponse.json() : [];
    // Room switches issue overlapping reads. A slow A response must never
    // repaint B's "This Loca" authority after the user has already moved,
    // nor a signed-out person's profile and rank come back after sign-out.
    if (sequence !== profileFetchSequence || room !== state.room || buildingStale(gen)) return;
    state.profile = profile;
    state.credentials = credentials;
    const priorPrincipal = state.principalId;
    state.principalId = profile.principal?.id || null;
    // Identity just hydrated. joinRoom() may have loaded the personal pin under
    // the name fallback before the real principal id arrived; re-key it now (if
    // still in a room and the principal actually changed) so a pin stored under
    // the principal key reappears after a slow /profile.
    if (state.room && state.principalId && state.principalId !== priorPrincipal) {
      loadPinned();
    }
    if (state.profile.principal?.display_name) {
      state.name = state.profile.principal.display_name;
      $("name").value = state.name;
    }
    if (state.principalId && state.principalId !== state.appearancePrincipal) {
      fetchAppearance();
    }
    markLocaContextReady("profile", room);
    loadRoomPreferences();
    renderRooms();
    renderProfile(now);
    renderLocaSidebar();
    renderOperatorControls();
  } catch (error) {
    if (sequence !== profileFetchSequence || room !== state.room || buildingStale(gen)) return;
    state.profile = null;
    state.credentials = [];
    if (profileRedrawMustWait(box, now)) {
      markLocaContextReady("profile", room);
      renderLocaSidebar();
      return;
    }
    const kept = accountMenuState(box);
    box.innerHTML = `<details class="profilemenu"><summary>` +
      `<span class="profileidentity"><span class="profileglyph">.</span>` +
      `<span><b>${esc(state.name)}</b><small>connected as ${esc(state.name)}</small></span></span>` +
      `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
      `<div class="profilepanel"><div class="profileempty">Profile is unavailable.</div>${profileServerHtml()}${profileSignOutHtml()}</div></details>`;
    restoreAccountMenu(box, kept);
    box.classList.add("on");
    markLocaContextReady("profile", room);
    renderLocaSidebar();
  }
}

async function createProfileCredential() {
  const input = $("credentialLabel");
  const label = input?.value.trim();
  if (!label) return;
  // The key made here is its asker's: its secret is shown to nobody who
  // signed in while the request, its body or the profile refresh was out.
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile/credentials`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ label }),
  });
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen)) addSys(`could not create access key: ${why}`);
    return;
  }
  const created = await response.json();
  if (buildingStale(gen)) return;
  await fetchProfile(true);
  if (buildingStale(gen)) return;
  $("whoami").querySelector("details.profilemenu").open = true;
  $("whoami").querySelector("details.profileaccess").open = true;
  const secret = $("credentialSecret");
  secret.classList.remove("hidden");
  secret.innerHTML = `<b>Copy now — shown once</b><code>${esc(created.secret)}</code>` +
    `<button type="button" id="credentialCopy">Copy</button>`;
  $("credentialCopy").onclick = async () => {
    await navigator.clipboard.writeText(created.secret);
    $("credentialCopy").textContent = "Copied";
  };
}

async function revokeProfileCredential(id, current) {
  const warning = current
    ? "Revoke the key behind this session? You will be signed out now."
    : "Revoke this access key? Other keys and the profile stay active.";
  if (!confirm(warning)) return;
  // Revoking one's own key ends one's own session. Answered after someone
  // else signed in, it must not end theirs.
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile/credentials/${encodeURIComponent(id)}`, {
    method: "DELETE", headers: adminHeaders({}),
  });
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen)) addSys(`could not revoke access key: ${why}`);
    return;
  }
  if (current) {
    try {
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (error) {}
    state.session = null;
    state.adminSession = false;
    state.sessionExpires = null;
    resetRoomPreferenceIdentity();
    state.profile = null;
    setLocked(true);
    clearBuildingData();
    clearPersonWords();
    setConnOpen(true);
    return;
  }
  await fetchProfile(true);
}

$("whoami").addEventListener("keydown", event => {
  if (event.key === "Enter" && event.target.id === "profileNameInput") renameProfile();
  if (event.key === "Enter" && event.target.id === "profileFullName") saveProfileCard();
});

$("whoami").addEventListener("click", event => {
  const create = event.target.closest("#credentialCreate");
  if (create) { createProfileCredential(); return; }
  if (event.target.closest("#profileRename")) { renameProfile(); return; }
  if (event.target.closest("#profileCardSave")) { saveProfileCard(); return; }
  const revoke = event.target.closest("[data-revoke-credential]");
  if (revoke) revokeProfileCredential(
    revoke.dataset.revokeCredential,
    revoke.dataset.current === "1",
  );
});

$("whoami").addEventListener("change", event => {
  const input = event.target.closest("input[data-appearance]");
  if (input?.checked) saveAppearance(input.dataset.appearance, input.value);
});

document.addEventListener("click", event => {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu?.open && !$("whoami").contains(event.target)) menu.open = false;
});
