"use strict";
// The Master Agent card in Building settings, for the Master only. A Master
// Agent is an automation (Legion's core, for one) that does three things and
// nothing else: it opens a loca on a floor named here, admits agents into
// the locas it opened, and sets or clears their lead. The feature is off by
// default; here the Master turns it on or off, names its floors, sets its
// limits, appoints a Master Agent (its key is shown once) and revokes one.
// A Smaster never sees this card, and the server refuses it these routes.

state.masterAgent = null; // { setting, agents, floors } as last read
state.masterAgentError = false;
state.masterAgentGen = null;
state.masterAgentAsk = null; // the id whose revocation is being asked

const MA_LIMITS = [
  ["locas_per_floor", "Locas on one floor"],
  ["locas_per_hour", "Locas opened per hour"],
  ["admissions_per_hour", "Agents admitted per hour"],
  ["live_agents", "Live agents it admitted"],
];

function maIsMaster() {
  return state.profile?.building_role === "master";
}

function maForgetAnother() {
  if (state.masterAgentGen === state.buildingGen) return;
  state.masterAgentGen = state.buildingGen;
  state.masterAgent = null;
  state.masterAgentError = false;
  state.masterAgentAsk = null;
}

async function fetchMasterAgent() {
  if (!maIsMaster()) return;
  maForgetAnother();
  const gen = state.buildingGen;
  try {
    const headers = { headers: adminHeaders({}) };
    const [rs, ra, rf] = await Promise.all([
      fetch(`${serverBase()}/building/settings/master-agent`, headers),
      fetch(`${serverBase()}/master-agents`, headers),
      fetch(`${serverBase()}/groups`, headers),
    ]);
    if (buildingStale(gen)) return;
    if (!rs.ok || !ra.ok || !rf.ok) {
      state.masterAgentError = true;
    } else {
      const [setting, agents, floors] = await Promise.all([rs.json(), ra.json(), rf.json()]);
      if (buildingStale(gen)) return;
      state.masterAgent = { setting, agents, floors };
      state.masterAgentError = false;
    }
  } catch (e) {
    if (buildingStale(gen)) return;
    state.masterAgentError = true;
  }
  renderMasterAgentCard();
}

function maWhen(ms) {
  return ms ? new Date(ms).toLocaleString([], { day: "2-digit", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" }) : "—";
}

function maAgentRow(agent) {
  if (state.masterAgentAsk === agent.id) {
    return `<li class="marow maask" data-ma-id="${esc(agent.id)}">` +
      `<span>Revoke <b>${esc(agent.name)}</b>? Its key and sessions end at once; the locas and agents it made stay.</span>` +
      `<span class="maactions"><button type="button" class="btn" data-ma-no>No</button>` +
      `<button type="button" class="btn danger" data-ma-yes="${esc(agent.id)}">Yes, revoke</button></span></li>`;
  }
  const revoked = agent.revoked_at != null;
  return `<li class="marow${revoked ? " marevoked" : ""}">` +
    `<span class="mawho"><span class="kindmark agent" aria-hidden="true"></span> <b>${esc(agent.name)}</b>` +
    ` <span class="matag">automation</span></span>` +
    `<span class="mameta">appointed ${esc(maWhen(agent.appointed_at))}` +
    ` · <code>${esc(agent.fingerprint)}</code>` +
    ` · ${revoked ? `revoked ${esc(maWhen(agent.revoked_at))}` : agent.last_used_at ? `last used ${esc(maWhen(agent.last_used_at))}` : "never used"}</span>` +
    (revoked ? "" : `<span class="maactions"><button type="button" class="btn" data-ma-revoke="${esc(agent.id)}">Revoke</button></span>`) +
    `</li>`;
}

function maLogLine(change) {
  const v = change.value || {};
  const floors = (v.floors || []).length;
  return `<li>${esc(maWhen(change.at))} · ${esc(change.by || "Master")}: ` +
    `${v.on ? "on" : "off"}, ${floors} floor${floors === 1 ? "" : "s"}</li>`;
}

function maCardHtml() {
  const head = `<h3 id="maH">Master Agent</h3>` +
    `<p class="bstext">An automation, such as Legion's core, that opens locas on the floors you name here, admits agents into them and sets their lead. It enters no other loca and does nothing else.</p>`;
  if (!state.masterAgent) {
    return head + (state.masterAgentError
      ? `<div class="bsnote bad" role="alert">The Master Agent settings could not be loaded.</div>`
      : `<div class="bsnote">Loading…</div>`);
  }
  const { setting, agents, floors } = state.masterAgent;
  const chosen = new Set(setting.floors || []);
  const live = (floors || []).filter(f => !f.archived);
  const floorBoxes = live.length
    ? live.map(f => `<label class="macheck"><input type="checkbox" name="maFloor" value="${esc(f.id)}"${chosen.has(f.id) ? " checked" : ""} /> ${esc(f.name)}</label>`).join("")
    : `<span class="bsdim">No floors yet. Create one, then name it here.</span>`;
  const ceiling = Number(setting.limit_ceiling) || 1000;
  const limits = MA_LIMITS.map(([key, label]) =>
    `<label class="malimit"><span>${esc(label)}</span>` +
    `<input type="number" name="${key}" min="1" max="${ceiling}" step="1" value="${esc(String(setting.limits?.[key] ?? ""))}" /></label>`).join("");
  const list = (agents || []).length
    ? `<ul class="malist">${agents.map(maAgentRow).join("")}</ul>`
    : `<p class="bsnote">No Master Agent has been appointed.</p>`;
  const log = (setting.log || []).slice(0, 10);
  return head +
    `<form id="maForm" class="maform" autocomplete="off">` +
      `<label class="maswitch"><input type="checkbox" id="maOn" role="switch"${setting.on ? " checked" : ""} />` +
      ` <span>${setting.on ? "On" : "Off"}</span></label>` +
      `<small class="bsnote">Off by default. While it is off no Master Agent key is issued and every Master Agent is refused; turning it on again restores the ones not revoked.</small>` +
      `<fieldset class="mafield"><legend>Floors it may open locas on</legend>${floorBoxes}` +
      `<small class="bsnote">A floor taken off keeps the locas it opened there, but gets no new loca or agent.</small></fieldset>` +
      `<fieldset class="mafield"><legend>Limits, for each Master Agent</legend><div class="malimits">${limits}</div></fieldset>` +
      `<div class="maline"><button type="submit" class="btn primary">Save</button>` +
      `<small class="bsnote" id="maNote" role="status" aria-live="polite"></small></div>` +
    `</form>` +
    `<h4 class="mah4">Master Agents</h4>${list}` +
    `<form id="maAppoint" class="bsnameline" autocomplete="off">` +
      `<input id="maName" maxlength="64" placeholder="Its name, e.g. legion-core" aria-label="Master Agent name"${setting.on ? "" : " disabled"} />` +
      `<button type="submit" class="btn primary"${setting.on ? "" : " disabled"}>Appoint</button></form>` +
    (setting.on ? "" : `<small class="bsnote">Turn it on to appoint one.</small>`) +
    `<div id="maIssued" class="maissued hidden" aria-live="polite"></div>` +
    (log.length ? `<details class="malog"><summary>Changes</summary><ul>${log.map(maLogLine).join("")}</ul></details>` : "");
}

// Drawn inside the Building settings grid, after its own cards.
function renderMasterAgentCard() {
  const grid = document.querySelector("#buildingSettings .bsgrid");
  if (!grid || !maIsMaster()) return;
  maForgetAnother();
  let card = $("maCard");
  if (!card) {
    card = document.createElement("section");
    card.id = "maCard";
    card.className = "bscard macard";
    card.setAttribute("aria-labelledby", "maH");
    grid.appendChild(card);
  }
  const issued = $("maIssued");
  const kept = issued && !issued.classList.contains("hidden") ? issued.innerHTML : "";
  card.innerHTML = maCardHtml();
  if (kept) {
    $("maIssued").innerHTML = kept;
    $("maIssued").classList.remove("hidden");
  }
}

function maNote(text, bad) {
  const note = $("maNote");
  if (!note) return;
  note.classList.toggle("bad", !!bad);
  note.textContent = text;
}

async function saveMasterAgentSetting(event) {
  event.preventDefault();
  const form = event.target;
  const gen = state.buildingGen;
  const limits = {};
  for (const [key] of MA_LIMITS) limits[key] = Number(form.elements[key].value);
  const body = {
    on: $("maOn").checked,
    floors: [...form.querySelectorAll("input[name=maFloor]:checked")].map(i => i.value),
    limits,
  };
  maNote("Saving…");
  try {
    const r = await fetch(`${serverBase()}/building/settings/master-agent`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(body),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const text = await r.text();
      if (!buildingStale(gen)) maNote(text || "Could not save.", true);
      return;
    }
    const setting = await r.json();
    if (buildingStale(gen) || !state.masterAgent) return;
    state.masterAgent.setting = setting;
    renderMasterAgentCard();
    maNote("Saved.");
  } catch (e) {
    if (!buildingStale(gen)) maNote("Could not reach the server.", true);
  }
}

async function appointMasterAgent(event) {
  event.preventDefault();
  const name = $("maName").value.trim();
  const gen = state.buildingGen;
  const box = $("maIssued");
  const r = await fetch(`${serverBase()}/keys`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ kind: "master_agent", name }),
  });
  if (buildingStale(gen)) return;
  if (!r.ok) {
    const text = await r.text();
    if (buildingStale(gen)) return;
    box.innerHTML = `<span class="bad">${esc(text)}</span>`;
    box.classList.remove("hidden");
    return;
  }
  const k = await r.json();
  // Signed out before the key arrived: it is never shown to the next person.
  if (buildingStale(gen)) return;
  box.innerHTML =
    `<span class="keyonce">Shown once — copy it now; it will not be shown again.</span>` +
    `<b class="keyfor">Key for Master Agent <b>${esc(k.name)}</b></b>` +
    `<div class="keydim">Put it in the automation's own secret store. Never give it to a model, a chat or a loca.</div>` +
    `<div class="keysecret"><code id="maSecret">${esc(k.code)}</code><button type="button" class="btn primary" data-ma-copy>Copy</button></div>` +
    `<div class="keydone"><button type="button" class="btn primary" data-ma-done>I've stored it</button></div>`;
  box.classList.remove("hidden");
  $("maName").value = "";
  await fetchMasterAgent();
}

async function revokeMasterAgent(id) {
  const gen = state.buildingGen;
  const r = await fetch(`${serverBase()}/master-agents/${encodeURIComponent(id)}`, {
    method: "DELETE",
    headers: adminHeaders({}),
  });
  if (buildingStale(gen)) return;
  state.masterAgentAsk = null;
  if (!r.ok) {
    const text = await r.text();
    if (buildingStale(gen)) return;
    renderMasterAgentCard();
    maNote(text || "Could not revoke.", true);
    return;
  }
  await fetchMasterAgent();
}

$("buildingSettings").addEventListener("submit", (event) => {
  if (event.target.id === "maForm") saveMasterAgentSetting(event);
  if (event.target.id === "maAppoint") appointMasterAgent(event);
});

$("buildingSettings").addEventListener("change", (event) => {
  if (event.target.id !== "maOn") return;
  const label = event.target.parentElement.querySelector("span");
  if (label) label.textContent = event.target.checked ? "On" : "Off";
});

$("buildingSettings").addEventListener("click", async (event) => {
  const ask = event.target.closest("[data-ma-revoke]");
  if (ask) {
    state.masterAgentAsk = ask.dataset.maRevoke;
    renderMasterAgentCard();
    $("maCard")?.querySelector("[data-ma-no]")?.focus();
    return;
  }
  if (event.target.closest("[data-ma-no]")) {
    state.masterAgentAsk = null;
    renderMasterAgentCard();
    return;
  }
  const yes = event.target.closest("[data-ma-yes]");
  if (yes) {
    yes.disabled = true;
    await revokeMasterAgent(yes.dataset.maYes);
    return;
  }
  if (event.target.closest("[data-ma-copy]")) {
    const code = $("maSecret")?.textContent || "";
    const button = event.target.closest("[data-ma-copy]");
    button.textContent = await copyText(code, $("maSecret")) ? "Copied" : "Selected — press Ctrl+C";
    return;
  }
  if (event.target.closest("[data-ma-done]")) {
    const box = $("maIssued");
    box.innerHTML = "";
    box.classList.add("hidden");
  }
});
