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
  davet: "Davet",
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
  try {
    const r = await fetch(`${serverBase()}/keys`, { headers: adminHeaders({}) });
    state.keyLedger = r.ok ? await r.json() : [];
  } catch (e) {
    state.keyLedger = [];
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
    const whom = k.kind === "davet" ? `${esc(k.for_name || "")} <span class="keyloca">#${esc(k.loca || "")}</span>` : esc(k.for_name || "—");
    const useBy = k.use_by && (k.state === "unused") ? `<span class="keydim">use by ${esc(keyWhen(k.use_by))}</span>` : "";
    const used = k.last_used_at ? `last used ${esc(keyWhen(k.last_used_at))}` : (k.state === "unused" ? "never used" : "");
    const act = revocable(k)
      ? `<button type="button" class="btn danger" data-key-revoke="${esc(k.id)}" data-key-label="${esc(KEY_KIND_LABEL[k.kind])} ${esc(k.fingerprint)}">Revoke</button>`
      : "";
    return `<div class="keyrow">` +
      `<span class="keykind"><b>${esc(KEY_KIND_LABEL[k.kind] || k.kind)}</b><code>${esc(k.fingerprint)}</code></span>` +
      `<span class="keywho">${whom}</span>` +
      `<span class="keydates">issued ${esc(keyWhen(k.issued_at))}${k.issued_by ? ` by ${esc(k.issued_by)}` : ""}<br>${useBy}${useBy && used ? " · " : ""}<span class="keydim">${used}</span></span>` +
      `<span class="keystate state-${esc(k.state)}">${esc(KEY_STATE_LABEL[k.state] || k.state)}</span>` +
      `<span class="pacts">${act}</span></div>`;
  }).join("") : `<div class="bbempty">No keys here.</div>`;
  box.innerHTML =
    `<div class="bbhead"><b>Key generator</b>` +
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
    `</div>${body}`;
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
  const r = await fetch(`${serverBase()}/keys`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(body),
  });
  if (!r.ok) { showIssued(`<span class="bad">${esc(await r.text())}</span>`); return; }
  const k = await r.json();
  if (k.kind === "admission") {
    showIssued(`<b>${k.minted} admission rights issued.</b> ${k.available} can be used now. People ask to join from the sign-in page; you approve them above.`);
  } else {
    const who = k.kind === "smaster" ? (k.name.startsWith("yeni-") ? "for a new Smaster" : `for <b>${esc(k.name)}</b>`) : "to sign in as Master";
    const life = k.kind === "smaster"
      ? `They choose Smaster at sign-in and type their own name. Use within 7 days (by ${esc(keyWhen(k.use_by))}); once used it works until revoked.`
      : `Use it once, by ${esc(keyWhen(k.use_by))}. The session it opens lasts ${k.session_hours} hours. Any earlier unused Master key stopped working.`;
    showIssued(
      `<div><b>Key ${who}</b> — copy it now; it will not be shown again.</div>` +
      `<div class="keysecret"><code id="keySecret">${esc(k.code)}</code><button type="button" class="btn" data-key-copy>Copy</button></div>` +
      `<div class="keydim">${life} Send it to the person directly, never in a shared channel.</div>`);
  }
  if (kind === "smaster") $("keyName").value = "";
  await fetchKeyLedger();
  fetchPeople();
}

async function keyLedgerClick(event) {
  const filter = event.target.closest("[data-key-filter]");
  if (filter) { state.keyFilter = filter.dataset.keyFilter; renderKeyLedger(); return; }
  if (event.target.closest("[data-key-copy]")) {
    const code = $("keySecret")?.textContent || "";
    try { await navigator.clipboard.writeText(code); event.target.textContent = "Copied"; }
    catch (e) { event.target.textContent = "Select and copy"; }
    return;
  }
  const revoke = event.target.closest("[data-key-revoke]");
  if (revoke) {
    if (!confirm(`Revoke ${revoke.dataset.keyLabel}? Whoever holds it can no longer use it.`)) return;
    const r = await fetch(`${serverBase()}/keys/${encodeURIComponent(revoke.dataset.keyRevoke)}`, {
      method: "DELETE",
      headers: adminHeaders({}),
    });
    if (!r.ok) showIssued(`<span class="bad">${esc(await r.text())}</span>`);
    await fetchKeyLedger();
    fetchPeople();
  }
}

// The Master names a Smaster from the Ranks list.
async function renameSmaster(current) {
  const name = prompt(`Real name for "${current}"?`, current);
  if (!name || name.trim() === current) return;
  const list = await (await fetch(`${serverBase()}/smasters`, { headers: adminHeaders({}) })).json();
  const row = list.find(s => s.name === current);
  if (!row) return;
  const r = await fetch(`${serverBase()}/smasters/${encodeURIComponent(row.id)}`, {
    method: "PUT",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ name: name.trim() }),
  });
  if (!r.ok) { alert(await r.text()); return; }
  fetchPeople();
}

$("keyLedger").addEventListener("click", (event) => keyLedgerClick(event));
$("keyLedger").addEventListener("change", (event) => { if (event.target.id === "keyKind") syncKeyForm(); });
$("keyLedger").addEventListener("submit", (event) => issueKey(event));
$("peopleList").addEventListener("click", (event) => {
  const rename = event.target.closest("[data-rename-smaster]");
  if (rename) renameSmaster(rename.dataset.renameSmaster);
});

// ---- Building sub-tabs: People · Keys · Private messages ----------------
// Keys and Private messages are the Master's alone (the server answers 403 to
// anyone else); for everyone else those tabs are not drawn at all.
state.buildingTab = "people";

function syncBuildingTabs() {
  const master = state.profile?.building_role === "master";
  $("btabKeys").classList.toggle("hidden", !master);
  $("btabPrivate").classList.toggle("hidden", !master);
  if (!master && state.buildingTab !== "people") state.buildingTab = "people";
  const panes = { people: "peopleList", keys: "keyLedger", private: "birebirOverview" };
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
  if (tab === "private") fetchBirebirOverview();
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
