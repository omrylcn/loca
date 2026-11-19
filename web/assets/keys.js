"use strict";
// The Master's key desk in the Building view: one place to issue a code, and
// the ledger of every code ever issued — when, for whom, until when it can be
// used, whether it was used, and what became of it. Secrets are shown once,
// right after they are issued, and never again; the ledger keeps only a
// fingerprint to recognise the code that was handed over.

state.keyLedger = [];
state.keyFilter = "all";

const KEY_KIND_LABEL = {
  master: "Master key",
  smaster: "Smaster key",
  member: "Membership",
  davet: "Invite",
  admission: "Admission right",
};
const KEY_STATE_LABEL = {
  unused: "unused",
  active: "in use",
  used: "used",
  expired: "expired",
  superseded: "replaced",
  revoked: "revoked",
};

async function fetchKeyLedger() {
  const box = $("keyLedger");
  if (!box) return;
  syncBuildingTabs();
  if (state.profile?.building_role !== "master") return;
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/keys`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    // A ledger that could not be read is unknown, not empty: "No keys here"
    // would tell the Master nothing was ever issued. What was last read stays
    // on screen, marked as possibly out of date.
    if (!r.ok) {
      state.keyLedgerError = true;
    } else {
      const ledger = await r.json();
      if (buildingStale(gen)) return;
      state.keyLedger = ledger;
      state.keyLedgerError = false;
    }
  } catch (e) {
    if (buildingStale(gen)) return;
    state.keyLedgerError = true;
  }
  renderKeyLedger();
}

function keyWhen(ms) {
  return ms ? new Date(ms).toLocaleString([], { day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit" }) : "—";
}

function renderKeyLedger() {
  const box = $("keyLedger");
  if (!box) return;
  const keepIssued = box.querySelector("#keyIssued")?.outerHTML || `<div class="keyissued hidden" id="keyIssued" role="status"></div>`;
  const filters = [
    ["all", "All"], ["unused", "Unused"], ["active", "In use"], ["used", "Used"], ["closed", "Expired · revoked"],
  ];
  const rows = state.keyLedger.filter(k => {
    if (state.keyFilter === "all") return true;
    if (state.keyFilter === "closed") return ["expired", "revoked", "superseded"].includes(k.state);
    return k.state === state.keyFilter;
  });
  const revocable = k =>
    (k.kind === "master" && k.state === "unused") ||
    (k.kind === "smaster" && (k.state === "unused" || k.state === "active")) ||
    (k.kind === "davet" && k.state === "active");
  const body = rows.length ? rows.map(k => {
    // The plain @name the key was issued for, and nothing more. A ledger row
    // records a name, not a person: after a revoke the same name can belong
    // to someone else, and their full name and icon must not appear on a key
    // that was never theirs.
    const holder = k.for_name ? esc(k.for_name) : "";
    const whom = k.kind === "davet" ? `${holder} <span class="keyloca">#${esc(k.loca || "")}</span>` : (holder || "—");
    const useBy = k.use_by && (k.state === "unused") ? `<span class="keydim">use by ${esc(keyWhen(k.use_by))}</span>` : "";
    const used = k.last_used_at ? `last used ${esc(keyWhen(k.last_used_at))}` : (k.state === "unused" ? "never used" : "");
    const act = revocable(k)
      ? `<button type="button" class="btn danger" data-key-revoke="${esc(k.id)}" data-key-label="${esc(KEY_KIND_LABEL[k.kind])} ${esc(k.fingerprint)}">Revoke</button>`
      : "";
    return `<div class="keyrow">` +
      `<span class="keywho">${whom}</span>` +
      `<span class="keykind"><b>${esc(KEY_KIND_LABEL[k.kind] || k.kind)}</b><code>${esc(k.fingerprint)}</code></span>` +
      `<span class="keydates">issued ${esc(keyWhen(k.issued_at))}${k.issued_by ? ` by ${esc(k.issued_by)}` : ""}<br>${useBy}${useBy && used ? " · " : ""}<span class="keydim">${used}</span></span>` +
      `<span class="keystate state-${esc(k.state)}">${esc(KEY_STATE_LABEL[k.state] || k.state)}</span>` +
      `<span class="pacts">${act}</span></div>`;
  }).join("") : (state.keyLedgerError ? "" : `<div class="bbempty">No keys here.</div>`);
  const unread = state.keyLedgerError
    ? `<div class="bbempty keyledgererror" role="alert">The key ledger could not be loaded.${rows.length ? " What is shown may be out of date." : ""}</div>`
    : "";
  box.innerHTML =
    `<div class="bbhead"><h2 class="bbtitle">Keys</h2><span class="bbtag">Master only</span>` +
    `<small>Every key that can open this building. Keys are never shown again; you can end one.</small></div>` +
    `<div class="keygenhead"><b>Key generator</b>` +
    `<small>Generate a key, copy it once, and every key ever generated stays recorded below.</small></div>` +
    `<form class="keyform" id="keyForm" autocomplete="off">` +
      `<select id="keyKind" aria-label="Kind of key">` +
        `<option value="master">Master key — sign in as Master</option>` +
        `<option value="smaster">Smaster key — for a person</option>` +
        `<option value="admission">Admission rights — let people join</option>` +
      `</select>` +
      `<input id="keyName" maxlength="64" placeholder="name (optional — they type it at sign-in)" aria-label="Person's name, optional" class="hidden" />` +
      `<button type="submit" class="btn primary">Generate key</button>` +
    `</form>` + keepIssued +
    `<div class="keylisthead">Generated keys <span>${state.keyLedger.length}</span> <small>for whom, when, until when, and what became of each — only the first characters are kept</small></div>` +
    `<div class="keyfilters" role="group" aria-label="Filter keys">` +
      filters.map(([v, l]) => `<button type="button" class="chip${state.keyFilter === v ? " on" : ""}" data-key-filter="${v}">${l}</button>`).join("") +
    `</div>${unread}` +
    (rows.length ? `<div class="keyhead" aria-hidden="true"><span>Belongs to</span><span>Kind</span><span>Issued · last used</span><span>State</span><span></span></div>` : "") +
    body;
  syncKeyForm();
}

function syncKeyForm() {
  const kind = $("keyKind")?.value;
  $("keyName")?.classList.toggle("hidden", kind !== "smaster");
}

function showIssued(html) {
  const box = $("keyIssued");
  box.innerHTML = html;
  box.classList.remove("hidden");
}

async function issueKey(event) {
  event.preventDefault();
  const kind = $("keyKind").value;
  const body = { kind };
  if (kind === "smaster") {
    body.name = $("keyName").value.trim();
  }
  const gen = state.buildingGen;
  const r = await fetch(`${serverBase()}/keys`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body),
  });
  if (buildingStale(gen)) return;
  if (!r.ok) {
    const text = await r.text();
    if (!buildingStale(gen)) showIssued(`<span class="bad">${esc(text)}</span>`);
    return;
  }
  const k = await r.json();
  // Signed out before the key arrived: it is never shown to the next person.
  if (buildingStale(gen)) return;
  if (k.kind === "admission") {
    showIssued(`<b>${k.minted} admission rights issued.</b> ${k.available} can be used now. People ask to join from the sign-in page; you approve them above.`);
  } else {
    const who = k.kind === "smaster" ? (k.name.startsWith("yeni-") ? "for a new Smaster" : `for <b>${esc(k.name)}</b>`) : "to sign in as Master";
    const life = k.kind === "smaster"
      ? `They choose Smaster at sign-in and type their own name. Use within 7 days (by ${esc(keyWhen(k.use_by))}); once used it works until revoked.`
      : `Use it once, by ${esc(keyWhen(k.use_by))}. The session it opens lasts ${k.session_hours} hours. Any earlier unused Master key stopped working.`;
    showIssued(
      `<span class="keyonce">Shown once — copy it now; it will not be shown again.</span>` +
      `<b class="keyfor">Key ${who}</b>` +
      `<div class="keydim">${life} Send it to the person directly, never in a shared channel.</div>` +
      `<div class="keysecret"><code id="keySecret">${esc(k.code)}</code><button type="button" class="btn primary" data-key-copy>Copy</button></div>` +
      `<div class="keywarn">This is the only time it is shown. Never paste it into a loca: everyone at the table would read it.</div>` +
      `<div class="keydone"><button type="button" class="btn primary" data-key-done>I've sent it</button></div>`);
  }
  if (kind === "smaster") $("keyName").value = "";
  await fetchKeyLedger();
  if (!buildingStale(gen)) fetchPeople();
}

async function keyLedgerClick(event) {
  const filter = event.target.closest("[data-key-filter]");
  if (filter) { state.keyFilter = filter.dataset.keyFilter; renderKeyLedger(); return; }
  if (event.target.closest("[data-key-done]")) {
    const issued = $("keyIssued");
    issued.innerHTML = "";
    issued.classList.add("hidden");
    return;
  }
  if (event.target.closest("[data-key-copy]")) {
    const code = $("keySecret")?.textContent || "";
    const button = event.target.closest("[data-key-copy]");
    button.textContent = await copyText(code, $("keySecret")) ? "Copied" : "Selected — press Ctrl+C";
    return;
  }
  const revoke = event.target.closest("[data-key-revoke]");
  if (revoke) {
    if (!confirm(`Revoke ${revoke.dataset.keyLabel}? Whoever holds it can no longer use it.`)) return;
    const gen = state.buildingGen;
    const r = await fetch(`${serverBase()}/keys/${encodeURIComponent(revoke.dataset.keyRevoke)}`, {
      method: "DELETE",
      headers: adminHeaders({}),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const text = await r.text();
      if (buildingStale(gen)) return;
      showIssued(`<span class="bad">${esc(text)}</span>`);
    }
    await fetchKeyLedger();
    if (!buildingStale(gen)) fetchPeople();
  }
}

// The Master removes a Smaster from People › Ranks. A Smaster names
// themselves at sign-in, so the Master has no Rename here. The question is
// asked in words under the row; only "Yes, remove" sends DELETE /smasters/:id.
function removeSmasterAskHtml(rank) {
  const question = rank.unused
    ? `Delete the unused Smaster code <b>${esc(rank.name)}</b>? Nobody will be able to sign in with it.`
    : `Remove <b>${esc(rank.name)}</b> from the Building? Their Smaster key stops working, they are signed out, and floors they founded pass to the Master.`;
  return `<div class="rankconfirm" role="alertdialog" aria-label="Remove ${esc(rank.name)}"><span>${question}</span>` +
    `<span class="rankconfirmacts"><button type="button" class="btn" data-remove-smaster-no>No</button>` +
    `<button type="button" class="btn danger" data-remove-smaster-yes="${esc(rank.name)}">Yes, remove</button></span></div>`;
}

async function removeSmaster(name, button) {
  // The removal is the Master's who asked: never sent, and never reported,
  // under whoever signed in while the list was being read.
  const gen = state.buildingGen;
  button.disabled = true;
  const listed = await fetch(`${serverBase()}/smasters`, { headers: adminHeaders({}) });
  const list = listed.ok ? await listed.json() : [];
  if (buildingStale(gen)) return;
  const row = (Array.isArray(list) ? list : []).find(s => s.name === name);
  const r = row
    ? await fetch(`${serverBase()}/smasters/${encodeURIComponent(row.id)}`, { method: "DELETE", headers: adminHeaders({}) })
    : null;
  if (buildingStale(gen)) return;
  state.removingSmaster = null;
  if (r && !r.ok) { const why = await r.text(); if (!buildingStale(gen)) alert(why); }
  fetchPeople();
}

$("keyLedger").addEventListener("click", (event) => keyLedgerClick(event));
$("keyLedger").addEventListener("change", (event) => { if (event.target.id === "keyKind") syncKeyForm(); });
$("keyLedger").addEventListener("submit", (event) => issueKey(event));
$("peopleList").addEventListener("click", (event) => {
  const remove = event.target.closest("[data-remove-smaster]");
  if (remove) { state.removingSmaster = { name: remove.dataset.removeSmaster, gen: state.buildingGen }; renderPeople(); return; }
  if (event.target.closest("[data-remove-smaster-no]")) { state.removingSmaster = null; renderPeople(); return; }
  const yes = event.target.closest("[data-remove-smaster-yes]");
  if (yes) removeSmaster(yes.dataset.removeSmasterYes, yes);
});

// ---- Building sub-tabs: People · Groups · Keys · Private messages --------
// Groups is for the ranks (each sees only the groups it may see). Keys and
// Private messages are the Master's alone (the server answers 403 to anyone
// else); for everyone else those tabs are not drawn at all.
state.buildingTab = "people";

function syncBuildingTabs() {
  const master = state.profile?.building_role === "master";
  const rank = master || state.profile?.building_role === "smaster";
  $("btabGroups").classList.toggle("hidden", !rank);
  $("btabKeys").classList.toggle("hidden", !master);
  $("btabPrivate").classList.toggle("hidden", !master);
  if (!master && (state.buildingTab === "keys" || state.buildingTab === "private")) state.buildingTab = "people";
  if (!rank && state.buildingTab === "groups") state.buildingTab = "people";
  const panes = { people: "peopleList", groups: "groupsPane", keys: "keyLedger", private: "dmOverview" };
  for (const [tab, pane] of Object.entries(panes)) {
    const on = state.buildingTab === tab;
    $(pane).classList.toggle("hidden", !on);
    const button = document.querySelector(`[data-btab="${tab}"]`);
    button.classList.toggle("active", on);
    button.setAttribute("aria-selected", String(on));
    button.tabIndex = on ? 0 : -1;
  }
}

function setBuildingTab(tab) {
  state.buildingTab = tab;
  syncBuildingTabs();
  if (tab === "keys") fetchKeyLedger();
  if (tab === "groups") fetchGroups();
  if (tab === "people") renderPeople();
  if (tab === "private") fetchDmOverview();
}

document.querySelector(".buildingtabs").addEventListener("click", (event) => {
  const tab = event.target.closest("[data-btab]");
  if (tab) setBuildingTab(tab.dataset.btab);
});
document.querySelector(".buildingtabs").addEventListener("keydown", (event) => {
  if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
  const tabs = [...document.querySelectorAll(".buildingtabs .btab:not(.hidden)")];
  const index = tabs.findIndex(t => t.dataset.btab === state.buildingTab);
  const next = tabs[(index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
  setBuildingTab(next.dataset.btab);
  next.focus();
});
