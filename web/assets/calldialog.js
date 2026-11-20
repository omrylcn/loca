"use strict";
// "+ call" in This Loca: a dialog, "Call into <loca>", in place of the list
// that opened squeezed under the button. Candidates are the Building's
// members who hold no seat here, grouped Agents and People; each row says
// whether they are online or away, where they sit, their floors and, for an
// agent, who admitted it (the membership's `admitted_by`, from GET /members:
// the server keeps no other owner for an agent). Several can be ticked and
// called at once: one POST /rooms/{loca}/call each, in turn, with a result
// for each. The rules are the old list's: suspended members are not
// offered, nor anyone seated here, and a floor loca offers only its floor's
// people. The server still decides every call.

let callDialog = null;

// What the server recorded as the admitting act, in words.
function callOwnerText(admittedBy) {
  if (typeof admittedBy !== "string" || !admittedBy) return "";
  if (admittedBy === "master") return "admitted by the Master";
  if (admittedBy === "master-console") return "admitted at the Master desk";
  if (admittedBy.startsWith("smaster:")) return `admitted by ${admittedBy.slice(8)} (Smaster)`;
  return `admitted by ${admittedBy}`;
}

function openCallDialog() {
  if (!state.room || !isAdmin()) return;
  callDialog = { room: state.room, gen: state.buildingGen, candidates: null, picked: new Set(), owners: new Map(),
    floor: null, results: null, calling: false, error: "" };
  $("callDialogTitle").textContent = `Call into ${locaName(state.room)}`;
  $("callSearch").value = "";
  renderCallDialog();
  openExtDialog("callDialog", "callSearch");
  loadCallCandidates(callDialog);
}

function closeCallDialog() {
  // Calls already on their way are seen through; nothing is left half-told.
  if (!callDialog || callDialog.calling) return;
  callDialog = null;
  closeExtDialog("callDialog");
}

function callDialogStale(dlg) {
  return callDialog !== dlg || buildingStale(dlg.gen) || state.room !== dlg.room;
}

async function loadCallCandidates(dlg) {
  try {
    const [res, mem] = await Promise.all([
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/members`, { headers: adminHeaders({}) }),
    ]);
    if (callDialogStale(dlg)) return;
    if (!res.ok) { dlg.error = "could not fetch the list"; renderCallDialog(); return; }
    const all = await res.json();
    // Who admitted whom is extra: without it the list still stands.
    const members = mem.ok ? await mem.json().catch(() => []) : [];
    if (callDialogStale(dlg)) return;
    dlg.owners = new Map((Array.isArray(members) ? members : []).map(m => [m.name, m.admitted_by]));
    // Floors: for the chips, and for the floor rule.
    await fetchGroups(false);
    if (callDialogStale(dlg)) return;
    const groupId = (state.rooms || []).find(rm => rm.room === dlg.room)?.group_id;
    let inGroup = null;
    if (groupId) {
      const group = (state.groups || []).find(g => g.id === groupId);
      dlg.floor = { name: group?.name || "this floor" };
      inGroup = new Set((group?.members || []).map(m => m.name));
    }
    const here = new Set((state.members || []).map(m => m.name));
    dlg.candidates = (Array.isArray(all) ? all : [])
      .filter(p => !p.suspended && !here.has(p.name) && !(p.locas || []).includes(dlg.room)
        && (!inGroup || inGroup.has(p.name)))
      .sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
  } catch (e) {
    if (callDialogStale(dlg)) return;
    dlg.error = "could not reach the server";
  }
  renderCallDialog();
}

// Where someone sits now: the first loca, and how many more, all of them in
// the tooltip.
function callWhereHtml(p) {
  const locas = (p.locas || []).filter(l => !isDm(l));
  if (!locas.length) return `<span class="callwhere">${p.online ? "in the Lobby" : "in no loca"}</span>`;
  const all = locas.map(l => `#${locaName(l)}`).join("\n");
  const more = locas.length > 1 ? `<span class="callmore">+${locas.length - 1}</span>` : "";
  return `<span class="callwhere" title="${esc(`seated in\n${all}`)}">in <span class="callloca">#${esc(locaName(locas[0]))}</span>${more}</span>`;
}

function callCandidateHtml(p, i) {
  const agent = p.kind === "agent";
  const owner = agent ? callOwnerText(callDialog.owners.get(p.name)) : "";
  const picked = callDialog.picked.has(p.name);
  const search = [p.name, p.full_name || ""].join(" ").toLowerCase();
  return `<label class="callcand" data-name="${esc(p.name)}" data-search="${esc(search)}">`
    + `<input type="checkbox" value="${esc(p.name)}"${picked ? " checked" : ""} aria-labelledby="callName${i}" aria-describedby="callMeta${i}">`
    + `<span class="callwho"><span class="callname" id="callName${i}">${kindMark(agent ? "agent" : "human")}${personLabelHtml(p.name, cardOf(p))}</span>`
    + `<span class="callpresence ${p.online ? "on" : "off"}">${p.online ? "online" : "away"}</span></span>`
    + `<span class="callmeta" id="callMeta${i}">${callWhereHtml(p)}<span class="callfloors">${groupChipsHtml(p.name)}</span>`
    + (owner ? `<span class="callowner">${esc(owner)}</span>` : "")
    + `</span></label>`;
}

function renderCallDialog() {
  const dlg = callDialog;
  if (!dlg) return;
  const body = $("callCands");
  const results = Array.isArray(dlg.results);
  $("callSearchField").classList.toggle("hidden", results);
  $("callFloorNote").textContent = dlg.floor && !results
    ? `Only ${dlg.floor.name}'s people can be called into this loca.` : "";
  if (results) {
    body.innerHTML = `<div class="callresults" role="list" aria-label="Results">${dlg.results.map(callResultHtml).join("")}</div>`;
  } else if (dlg.error) {
    body.innerHTML = `<div class="callnone">${esc(dlg.error)}</div>`;
  } else if (!dlg.candidates) {
    body.innerHTML = `<div class="callnone">looking…</div>`;
  } else if (!dlg.candidates.length) {
    body.innerHTML = `<div class="callnone">${dlg.floor
      ? "Nobody else on this floor to call. Add them to the floor first."
      : "Nobody in the Building to call."}</div>`;
  } else {
    const groups = [
      ["Agents", dlg.candidates.filter(p => p.kind === "agent")],
      ["People", dlg.candidates.filter(p => p.kind !== "agent")],
    ].filter(([, rows]) => rows.length);
    let i = 0;
    body.innerHTML = groups.map(([label, rows]) =>
      `<section class="callgroup"><h3>${label} <span class="bcount">${rows.length}</span></h3>`
      + rows.map(p => callCandidateHtml(p, i++)).join("") + `</section>`).join("")
      + `<div class="callnone callnomatch hidden">Nobody matches.</div>`;
    filterCallCandidates();
  }
  renderCallFooter();
}

function filterCallCandidates() {
  const q = $("callSearch").value.trim().toLowerCase();
  let shown = 0;
  for (const group of document.querySelectorAll("#callCands .callgroup")) {
    let inGroup = 0;
    for (const row of group.querySelectorAll(".callcand")) {
      const match = !q || row.dataset.search.includes(q);
      row.hidden = !match;
      if (match) inGroup++;
    }
    group.hidden = !inGroup;
    const count = group.querySelector(".bcount");
    if (count) count.textContent = q ? `${inGroup} of ${group.querySelectorAll(".callcand").length}` : String(inGroup);
    shown += inGroup;
  }
  document.querySelector("#callCands .callnomatch")?.classList.toggle("hidden", shown > 0);
  if (callDialog) renderCallFooter();
}

function renderCallFooter() {
  const dlg = callDialog;
  const results = Array.isArray(dlg.results);
  const n = dlg.picked.size;
  $("callSubmit").classList.toggle("hidden", results);
  $("callSubmit").disabled = !n || dlg.calling;
  $("callSubmit").textContent = n ? `Call ${n}` : "Call";
  $("callAgain").classList.toggle("hidden", !results || dlg.calling);
  $("callCancel").textContent = results ? "Done" : "Cancel";
  $("callCancel").disabled = dlg.calling;
  const summary = $("callSummary");
  if (!results) {
    // A tick the search hides is still a tick: say so before "Call N".
    const hidden = [...document.querySelectorAll("#callCands .callcand[hidden] input:checked")].length;
    summary.textContent = !n ? "" : hidden ? `${n} selected (${hidden} hidden by the search)` : `${n} selected`;
    return;
  }
  const count = s => dlg.results.filter(r => r.state === s).length;
  const called = count("called"), already = count("already"), failed = count("failed");
  summary.textContent = dlg.calling ? `calling ${dlg.results.length - count("pending") + 1} of ${dlg.results.length}…`
    : `${called} of ${dlg.results.length} called in.`
      + (already ? ` ${already} already had a seat.` : "")
      + (failed ? ` ${failed} could not be called.` : "");
}

function callResultHtml(r) {
  const what = r.state === "pending" ? "waiting…"
    : r.state === "called" ? "called in"
    : r.state === "already" ? (r.reissued || "already has a seat here")
    : `not called: ${r.why}`;
  // Someone seated in the meantime may have lost the invitation behind the
  // seat: the same re-send as the old list offered (people.js reissueInvite).
  const resend = r.state === "already" && !r.reissued
    ? `<button type="button" class="btn" data-call-dialog-reissue="${esc(r.name)}">Re-send invitation</button>` : "";
  return `<div class="callresult ${r.state}" role="listitem"><span class="callresultname">${esc(r.name)}</span>`
    + `<span class="callresultwhat">${esc(what)}</span>${resend}</div>`;
}

// One request per person, in turn, through the call route.
async function callOneIntoLoca(room, name) {
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/call`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name }),
    });
    if (r.ok) {
      const v = await r.json().catch(() => ({}));
      return v.already ? { state: "already" } : { state: "called" };
    }
    if (r.status === 404) return { state: "failed", why: "not a Building member" };
    return { state: "failed", why: (await r.text().catch(() => "")) || `refused (${r.status})` };
  } catch (e) {
    return { state: "failed", why: "could not reach the server" };
  }
}

async function callPicked() {
  const dlg = callDialog;
  if (!dlg || dlg.calling || !dlg.picked.size) return;
  const names = (dlg.candidates || []).map(p => p.name).filter(n => dlg.picked.has(n));
  dlg.calling = true;
  dlg.results = names.map(name => ({ name, state: "pending" }));
  renderCallDialog();
  for (const result of dlg.results) {
    const outcome = await callOneIntoLoca(dlg.room, result.name);
    if (buildingStale(dlg.gen)) return;
    Object.assign(result, outcome);
    // The same lines in the conversation as one call made before.
    if (result.state === "called") addSys(`${result.name} called into this loca`);
    else if (result.state === "already") addSys(`${result.name} already has a seat in this loca`);
    if (callDialog === dlg) renderCallDialog();
  }
  dlg.calling = false;
  dlg.picked.clear();
  if (callDialog === dlg) {
    renderCallDialog();
    $("callCancel").focus();
  }
  refreshRooms();
  fetchLobby();
  fetchSeated();
}

// Keep Tab and Shift+Tab inside the dialog.
function trapCallDialogFocus(event) {
  const focusable = [...$("callDialog").querySelectorAll("button, input, select, textarea, [href], [tabindex]:not([tabindex='-1'])")]
    .filter(el => !el.disabled && el.tabIndex !== -1 && el.getClientRects().length && !el.closest("[hidden], .hidden"));
  if (!focusable.length) return;
  const first = focusable[0], last = focusable[focusable.length - 1];
  const inside = $("callDialog").contains(document.activeElement);
  if (event.shiftKey && (document.activeElement === first || !inside)) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && (document.activeElement === last || !inside)) { event.preventDefault(); first.focus(); }
}

$("callSearch").addEventListener("input", filterCallCandidates);
$("callCands").addEventListener("change", (event) => {
  const box = event.target.closest("input[type='checkbox']");
  if (!box || !callDialog) return;
  if (box.checked) callDialog.picked.add(box.value); else callDialog.picked.delete(box.value);
  renderCallFooter();
});
$("callCands").addEventListener("click", async (event) => {
  const resend = event.target.closest("[data-call-dialog-reissue]");
  const dlg = callDialog;
  if (!resend || !dlg) return;
  const name = resend.dataset.callDialogReissue;
  resend.disabled = true;
  const sent = await reissueInvite(name, dlg.room);
  if (callDialog !== dlg) return;
  const result = dlg.results.find(r => r.name === name);
  if (result) result.reissued = sent === "sent" ? "fresh invitation sent"
    : sent === "unchanged" ? "this server cannot re-send invitations yet" : "";
  renderCallDialog();
});
$("callSubmit").onclick = callPicked;
$("callAgain").onclick = () => {
  const dlg = callDialog;
  if (!dlg || dlg.calling) return;
  dlg.results = null;
  dlg.candidates = null;
  $("callSearch").value = "";
  renderCallDialog();
  $("callSearch").focus();
  loadCallCandidates(dlg);
};
$("callCancel").onclick = closeCallDialog;
$("callDialogBackdrop").onclick = closeCallDialog;
$("callDialogClose").onclick = closeCallDialog;
$("callDialog").addEventListener("keydown", (event) => {
  if (event.key === "Escape") { event.preventDefault(); closeCallDialog(); }
  else if (event.key === "Tab") trapCallDialogFocus(event);
});
