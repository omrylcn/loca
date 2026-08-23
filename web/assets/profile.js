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
    : loca?.operator_source?.startsWith("inherited") ? "inherited" : "";
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
    `<div class="profileconnected">connected as <b>${esc(principal.display_name)}</b> · You · ${profileKindLabel(principal.kind)}</div>` +
    `<div class="profileroles">` +
      `<span><small>Building</small><b class="role building-${esc(profile.building_role)}">${profileRoleLabel(profile.building_role)}</b></span>` +
      `<span><small>This Loca</small><b class="role loca-role">${esc(locaText)}</b>${source ? `<em>${source}</em>` : ""}</span>` +
    `</div>` +
    `<div class="profilesession"><small>Session</small><span>${profile.session ? "Bounded session" : "Direct credential"} · ${esc(expiry)}</span></div>` +
    `<details class="profileaccess"><summary>Your access <span>${state.credentials.filter(c => !c.revoked_at).length}</span></summary>` +
      `<div class="credentiallist">${credentials || '<div class="credentialnone">No access keys listed.</div>'}</div>` +
      `<div class="credentialcreate"><input id="credentialLabel" maxlength="64" placeholder="New key label, e.g. MacBook" />` +
      `<button type="button" id="credentialCreate">Create key</button></div>` +
      `<div class="credentialsecret hidden" id="credentialSecret" role="status"></div>` +
    `</details></div></details>`;
  box.classList.add("on");
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
    state.principalId = profile.principal?.id || null;
    if (state.profile.principal?.display_name) {
      state.name = state.profile.principal.display_name;
      $("name").value = state.name;
    }
    markLocaContextReady("profile", room);
    loadRoomPreferences();
    renderRooms();
    renderProfile();
    renderLocaSidebar();
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

$("whoami").addEventListener("click", event => {
  const create = event.target.closest("#credentialCreate");
  if (create) { createProfileCredential(); return; }
  const revoke = event.target.closest("[data-revoke-credential]");
  if (revoke) revokeProfileCredential(
    revoke.dataset.revokeCredential,
    revoke.dataset.current === "1",
  );
});

document.addEventListener("click", event => {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu?.open && !$("whoami").contains(event.target)) menu.open = false;
});
