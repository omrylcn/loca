"use strict";
// Server-derived identity, composed Building/Loca roles, and personal access.

function profileRoleLabel(role) {
  return String(role || "member").replaceAll("_", " ").toUpperCase();
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
  density: { comfortable: "Comfortable", compact: "Compact" },
  font_size: { normal: "Normal", large: "Large" },
};
const APPEARANCE_ROWS = [["theme", "Theme"], ["accent", "Accent"], ["density", "Density"], ["font_size", "Text"]];

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
      const face = key === "accent"
        ? `<span class="swatch ${value}" title="${name}" aria-hidden="true"></span><span class="visuallyhidden">${name}</span>`
        : `<span>${name}</span>`;
      return `<label class="appearanceopt"><input type="radio" name="appearance-${key}"` +
        ` data-appearance="${key}" value="${value}"${checked} />${face}</label>`;
    }).join("");
    return `<div class="appearancerow" role="radiogroup" aria-labelledby="${labelId}">` +
      `<span id="${labelId}">${title}</span><div class="appearanceopts">${options}</div></div>`;
  }).join("");
  return `<div class="profileappearance" id="profileAppearance"><small>Appearance</small>${rows}` +
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
    if (sequence === appearanceSaveSequence) setAppearanceStatus("Not saved to your profile; kept on this device.");
  }
}

function renderProfile() {
  const box = $("whoami");
  const profile = state.profile;
  if (!profile?.principal) {
    box.innerHTML = `<details class="profilemenu"><summary>` +
      `<span class="profileidentity"><span class="profileglyph">.</span>` +
      `<span><b>${esc(state.name)}</b><small>connected as ${esc(state.name)}</small></span></span>` +
      `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
      `<div class="profilepanel"><div class="profileempty">Profile is available after you take a seat.</div></div></details>`;
    box.classList.add("on");
    return;
  }
  const principal = profile.principal;
  const loca = profile.loca;
  const roles = loca?.roles || [];
  const locaLabels = roles
    .filter(role => role !== "participant" || roles.length === 1)
    .map(profileRoleLabel);
  const source = loca?.operator_source === "appointed" ? "appointed"
    : loca?.operator_source === "master" ? "by rank (Master)" : "";
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
  box.innerHTML = `<details class="profilemenu"><summary aria-label="Open your profile">` +
    `<span class="profileidentity"><span class="profileglyph">${principal.kind === "agent" ? "*" : "."}</span>` +
      `<span><b>${esc(principal.display_name)}</b><small>You · ${profileKindLabel(principal.kind)}</small></span></span>` +
    `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
    `<div class="profilepanel"><div class="profilepanelhead"><span>Profile</span><small>Identity &amp; access</small></div>` +
    `<div class="profileconnected">connected as <b>${esc(principal.display_name)}</b>${rankBadge(profile.building_role)} · You · ${profileKindLabel(principal.kind)}</div>` +
    profileRenameHtml(profile, principal) +
    `<div class="profileroles">` +
      `<span><small>Building</small><b class="role building-${esc(profile.building_role)}">${profileRoleLabel(profile.building_role)}</b></span>` +
      `<span><small>This Loca</small><b class="role loca-role">${esc(locaText)}</b>${source ? `<em>${source}</em>` : ""}${management ? `<em>${management}</em>` : ""}</span>` +
    `</div>` +
    `<div class="profilesession"><small>Session</small><span>${profile.session ? "Bounded session" : "Direct credential"} · ${esc(expiry)}</span></div>` +
    profileAppearanceHtml() +
    `<details class="profileaccess"><summary>Your access <span>${state.credentials.filter(c => !c.revoked_at).length}</span></summary>` +
      `<div class="credentiallist">${credentials || '<div class="credentialnone">No access keys listed.</div>'}</div>` +
      `<div class="credentialcreate"><input id="credentialLabel" maxlength="64" placeholder="New key label, e.g. MacBook" />` +
      `<button type="button" id="credentialCreate">Create key</button></div>` +
      `<div class="credentialsecret hidden" id="credentialSecret" role="status"></div>` +
    `</details></div></details>`;
  box.classList.add("on");
}

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

// Set the display name chosen at sign-in. Returns "" on success, or the
// server's reason when it is refused (a title, or a name someone else wears).
async function applyChosenName(name) {
  try {
    const response = await fetch(`${serverBase()}/profile`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ display_name: name }),
    });
    if (!response.ok) return (await response.text()) || "the name was refused";
    adoptOwnName((await response.json()).display_name);
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
  const response = await fetch(`${serverBase()}/profile`, {
    method: "PUT",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ display_name: name }),
  });
  if (!response.ok) {
    if (note) note.textContent = await response.text();
    return;
  }
  const renamed = await response.json();
  const before = state.name;
  adoptOwnName(renamed.display_name);
  // The server relabels history and tells every open feed; reconnecting
  // here covers a loca with nothing said yet (no frame arrives).
  if (before !== renamed.display_name && state.room) openWs();
  await fetchProfile();
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const after = $("profileRenameNote");
  if (after) after.textContent = `Renamed to ${renamed.display_name}.`;
}

let profileFetchSequence = 0;
async function fetchProfile() {
  const box = $("whoami");
  const room = state.room;
  const sequence = ++profileFetchSequence;
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
    // repaint B's "This Loca" authority after the user has already moved.
    if (sequence !== profileFetchSequence || room !== state.room) return;
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
    renderProfile();
    renderLocaSidebar();
    renderOperatorControls();
  } catch (error) {
    if (sequence !== profileFetchSequence || room !== state.room) return;
    state.profile = null;
    state.credentials = [];
    box.innerHTML = `<details class="profilemenu"><summary>` +
      `<span class="profileidentity"><span class="profileglyph">.</span>` +
      `<span><b>${esc(state.name)}</b><small>connected as ${esc(state.name)}</small></span></span>` +
      `<span class="profilechevron" aria-hidden="true">›</span></summary>` +
      `<div class="profilepanel"><div class="profileempty">Profile is unavailable.</div></div></details>`;
    box.classList.add("on");
    markLocaContextReady("profile", room);
    renderLocaSidebar();
  }
}

async function createProfileCredential() {
  const input = $("credentialLabel");
  const label = input?.value.trim();
  if (!label) return;
  const response = await fetch(`${serverBase()}/profile/credentials`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ label }),
  });
  if (!response.ok) {
    addSys(`could not create access key: ${await response.text()}`);
    return;
  }
  const created = await response.json();
  await fetchProfile();
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
  const response = await fetch(`${serverBase()}/profile/credentials/${encodeURIComponent(id)}`, {
    method: "DELETE", headers: adminHeaders({}),
  });
  if (!response.ok) {
    addSys(`could not revoke access key: ${await response.text()}`);
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
    setConnOpen(true);
    return;
  }
  await fetchProfile();
}

$("whoami").addEventListener("keydown", event => {
  if (event.key === "Enter" && event.target.id === "profileNameInput") renameProfile();
});

$("whoami").addEventListener("click", event => {
  const create = event.target.closest("#credentialCreate");
  if (create) { createProfileCredential(); return; }
  if (event.target.closest("#profileRename")) { renameProfile(); return; }
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
