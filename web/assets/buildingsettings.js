"use strict";
// Building settings (BuildingSettings.dc.html): the Building's Settings tab,
// for the Master and Smasters (GET /building/settings). It offers only what
// the server lets someone change at runtime, and that is one thing: the
// Building's name, which only the Master changes (PUT /building/settings).
// Everything else comes from the server's environment at start-up and is
// shown read-only under its variable's name, with the value in force. A
// Smaster sees the same page with nothing to change.

state.buildingSettings = null;
state.buildingSettingsError = false;
// The generation the settings were read under: another person signing in
// never sees what was read for the one before.
state.buildingSettingsGen = null;

function forgetBuildingSettingsOfAnother() {
  if (state.buildingSettingsGen === state.buildingGen) return;
  state.buildingSettingsGen = state.buildingGen;
  state.buildingSettings = null;
  state.buildingSettingsError = false;
}

async function fetchBuildingSettings() {
  const box = $("buildingSettings");
  if (!box) return;
  const role = state.profile?.building_role;
  if (role !== "master" && role !== "smaster") return;
  forgetBuildingSettingsOfAnother();
  if (!state.buildingSettings) renderBuildingSettings();
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/building/settings`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      state.buildingSettingsError = true;
    } else {
      const settings = await r.json();
      if (buildingStale(gen)) return;
      state.buildingSettings = settings;
      state.buildingSettingsError = false;
    }
  } catch (e) {
    if (buildingStale(gen)) return;
    state.buildingSettingsError = true;
  }
  renderBuildingSettings();
}

// A value the environment sets, with the variable that sets it.
function bsEnvRow(label, value, variable) {
  return `<div class="bsrow"><dt>${esc(label)}</dt>` +
    `<dd><span class="bsvalue">${value}</span> <code class="bsvar">${esc(variable)}</code></dd></div>`;
}

function bsBackupDocUrl(settings) {
  const version = String(settings.version || "").replace(/[^0-9A-Za-z.+-]/g, "");
  const ref = version ? `v${version}` : "main";
  return `https://github.com/omrylcn/loca/blob/${ref}/${settings.backup?.doc || "docs/backup-restore.md"}`;
}

function renderBuildingSettings() {
  const box = $("buildingSettings");
  if (!box) return;
  forgetBuildingSettingsOfAnother();
  const s = state.buildingSettings;
  const master = state.profile?.building_role === "master";
  const head =
    `<div class="bbhead"><h2 class="bbtitle">Building settings</h2>` +
    `<span class="bbtag">${master ? "Master only" : "Read only"}</span>` +
    `<small>${master
      ? "The name is yours to change here. The rest is set in the server's environment and changes when it is edited and the server restarts."
      : "Only the Master changes the Building's name. The rest is set in the server's environment."}</small></div>`;
  if (!s) {
    box.innerHTML = head + (state.buildingSettingsError
      ? `<div class="bbempty" role="alert">The Building settings could not be loaded.</div>`
      : `<div class="bbempty">Loading…</div>`);
    return;
  }
  const env = s.environment || {};
  const canName = master && Array.isArray(s.editable) && s.editable.includes("name");
  const address = env.PUBLIC_SERVER_URL
    ? `<span class="bsmono">${esc(env.PUBLIC_SERVER_URL)}</span>`
    : `<span class="bsdim">not set</span>`;
  const name = canName
    ? `<form class="bsname" id="bsNameForm" autocomplete="off">` +
        `<label for="bsName">Name</label>` +
        `<div class="bsnameline"><input id="bsName" maxlength="${Number(s.name_max_chars) || 64}" value="${esc(s.name || "")}" placeholder="This Building has no name yet" />` +
        `<button type="submit" class="btn primary">Save</button></div>` +
        `<small class="bsnote" id="bsNameNote" role="status" aria-live="polite"></small>` +
      `</form>`
    : `<dl class="bslist"><div class="bsrow"><dt>Name</dt><dd>${s.name ? esc(s.name) : `<span class="bsdim">no name yet</span>`}</dd></div></dl>`;
  const caretakers = (env.LOCA_CARETAKERS || []).map(n =>
    `<span class="bsagent"><span class="kindmark agent" aria-hidden="true"></span> ${esc(n)}</span>`).join(", ");
  const storage = s.persistent
    ? `kept on disk`
    : `<span class="bswarn">memory only: everything clears on restart</span>`;

  box.innerHTML = head + `<div class="bsgrid">` +
    `<section class="bscard" aria-labelledby="bsBuildingH"><h3 id="bsBuildingH">The building</h3>${name}` +
      `<dl class="bslist">` +
        bsEnvRow("Address", address, "PUBLIC_SERVER_URL") +
        `<div class="bsrow"><dt>Version</dt><dd><span class="bsvalue">${esc(s.version || "")}</span></dd></div>` +
      `</dl></section>` +
    `<section class="bscard" aria-labelledby="bsDoorH"><h3 id="bsDoorH">Who can get in</h3>` +
      `<p class="bstext">People and agents can ask to join; nobody enters until the Master or a Smaster approves.</p>` +
      `<dl class="bslist">` +
        bsEnvRow("Invitation only", env.REQUIRE_INVITE
          ? "on — every loca door needs its own invite"
          : "off — a shared Building key also opens loca doors, if one is set", "REQUIRE_INVITE") +
        bsEnvRow("Sessions required", env.REQUIRE_SESSIONS
          ? "on — who posts is always the signed-in identity"
          : "off", "REQUIRE_SESSIONS") +
      `</dl></section>` +
    `<section class="bscard" aria-labelledby="bsLocasH"><h3 id="bsLocasH">Locas</h3>` +
      `<dl class="bslist">` +
        bsEnvRow("Seats by default", esc(String(env.LOCA_CAPACITY ?? "")), "LOCA_CAPACITY") +
        bsEnvRow("At most", esc(String(env.LOCA_CAPACITY_MAX ?? "")), "LOCA_CAPACITY_MAX") +
        bsEnvRow("Messages per loca, to start", `${esc(String(env.RATE_LIMIT ?? ""))} every ${esc(String(env.RATE_WINDOW_SECS ?? ""))} s`, "RATE_LIMIT · RATE_WINDOW_SECS") +
        bsEnvRow("Notifications per sender", `${esc(String(env.NOTIFICATION_RATE_LIMIT ?? ""))} every 10 minutes`, "NOTIFICATION_RATE_LIMIT") +
      `</dl>` +
      `<p class="bsnote">A seat is one person or one agent; someone only viewing takes none.</p></section>` +
    `<section class="bscard" aria-labelledby="bsCareH"><h3 id="bsCareH">Caretakers and backups</h3>` +
      `<dl class="bslist">` +
        bsEnvRow("Caretakers", caretakers || `<span class="bsdim">none</span>`, "LOCA_CARETAKERS") +
        bsEnvRow("Their loca", `<span class="bsmono">${esc(env.LOCA_AGENT_ROOM || "")}</span>`, "LOCA_AGENT_ROOM") +
        bsEnvRow("Storage", storage, "DB_PATH") +
      `</dl>` +
      `<p class="bstext">A backup is taken on the server's host with the server stopped, database and attachments together. There is no download of the whole Building here.</p>` +
      `<a class="bslink" href="${esc(bsBackupDocUrl(s))}" target="_blank" rel="noopener noreferrer">Backup and restore (docs/backup-restore.md)</a>` +
    `</section>` +
  `</div>`;
}

async function saveBuildingName(event) {
  event.preventDefault();
  const input = $("bsName");
  const note = $("bsNameNote");
  const button = event.target.querySelector("button[type=submit]");
  const gen = state.buildingGen;
  button.disabled = true;
  note.classList.remove("bad");
  note.textContent = "Saving…";
  try {
    const r = await fetch(`${serverBase()}/building/settings`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name: input.value }),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const text = await r.text();
      if (buildingStale(gen)) return;
      note.classList.add("bad");
      note.textContent = text || "Could not save the name.";
      return;
    }
    const saved = await r.json();
    if (buildingStale(gen)) return;
    if (state.buildingSettings) state.buildingSettings.name = saved.name;
    input.value = saved.name || "";
    note.textContent = saved.name ? "Saved." : "Saved: the Building has no name.";
  } catch (e) {
    if (buildingStale(gen)) return;
    note.classList.add("bad");
    note.textContent = "Could not reach the server.";
  } finally {
    button.disabled = false;
  }
}

$("buildingSettings").addEventListener("submit", (event) => {
  if (event.target.id === "bsNameForm") saveBuildingName(event);
});
