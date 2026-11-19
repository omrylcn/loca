"use strict";
// The Vault tab (the `vault` extension): the loca's rules document. Everyone
// seated reads it; the server says who may write it (`can_edit`: the loca's
// Operator, the Master and Smasters). It informs; it grants nothing.
let vaultDoc = null;
let vaultHistory = null;
let vaultFetchSequence = 0;

// The History card: each kept revision, newest first ("v4 · today · Ömer").
function renderVaultHistory() {
  const box = $("vaultHistory");
  const revisions = vaultHistory && vaultHistory.room === state.room ? vaultHistory.revisions : [];
  if (!revisions.length) {
    box.innerHTML = `<span class="pagemuted">No revisions yet.</span>`;
    return;
  }
  box.innerHTML = revisions.map((doc, i) => {
    const when = doc.updated_at ? dayInLine(doc.updated_at) : "";
    const line = [`v${doc.rev}`, when, doc.updated_by || ""].filter(Boolean).join(" · ");
    return `<span class="${i ? "pagemuted" : ""}" title="${esc(doc.updated_at ? fmtFull(doc.updated_at) : "")}">${esc(line)}</span>`;
  }).join("");
}

async function fetchVaultHistory(room, sequence) {
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/vault/history`, { headers: adminHeaders({}) });
    if (!response.ok) return;
    const answer = await response.json();
    if (room !== state.room || sequence !== vaultFetchSequence) return;
    vaultHistory = { room, revisions: Array.isArray(answer.revisions) ? answer.revisions : [] };
    renderVaultHistory();
  } catch (error) { /* the history card is a convenience; the rules still show */ }
}

function renderVault() {
  const doc = vaultDoc && vaultDoc.room === state.room ? vaultDoc : null;
  const body = $("vaultBody");
  if (!doc) {
    body.replaceChildren(); $("vaultEdit").classList.add("hidden"); $("vaultEditJump").classList.add("hidden");
    renderVaultHistory();
    return;
  }
  $("vaultStatus").textContent = doc.rev
    ? `Revision ${doc.rev} · written by ${doc.updated_by} · ${fmtFull(doc.updated_at)}`
    : "No rules yet.";
  if (doc.body) body.innerHTML = renderMarkdown(doc.body);
  else body.textContent = doc.can_edit
    ? "This loca has no rules yet. Write them below."
    : "This loca has no rules yet.";
  $("vaultEdit").classList.toggle("hidden", !doc.can_edit);
  $("vaultEditJump").classList.toggle("hidden", !doc.can_edit);
  renderVaultHistory();
  // An edit in progress is kept when another revision arrives; Save then
  // answers 409 and asks to reload.
  if (doc.can_edit && !$("vaultText").dataset.dirty) {
    $("vaultText").value = doc.body;
    $("vaultText").dataset.rev = String(doc.rev);
  }
}

async function fetchVault() {
  if (!state.room || !extensionOn("vault")) return;
  const room = state.room;
  const sequence = ++vaultFetchSequence;
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/vault`, { headers: adminHeaders({}) });
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    const doc = await response.json();
    if (room !== state.room || sequence !== vaultFetchSequence) return;
    if (vaultDoc?.room !== room) delete $("vaultText").dataset.dirty;
    vaultDoc = doc;
    renderVault();
    fetchVaultHistory(room, sequence);
  } catch (error) {
    if (room === state.room && !buildingStale(gen)) $("vaultStatus").textContent = String(error.message || error);
  }
}

async function saveVault() {
  const room = state.room;
  if (!vaultDoc || vaultDoc.room !== room || !vaultDoc.can_edit) return;
  const status = $("vaultEditStatus");
  $("vaultSave").disabled = true;
  status.textContent = "Saving…";
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/vault`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ body: $("vaultText").value, base_rev: Number($("vaultText").dataset.rev || 0) }),
    });
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    const doc = await response.json();
    if (room !== state.room) return;
    delete $("vaultText").dataset.dirty;
    vaultDoc = doc;
    renderVault();
    status.textContent = `Saved · revision ${doc.rev}`;
    fetchVaultHistory(room, vaultFetchSequence);
  } catch (error) {
    if (room === state.room) status.textContent = String(error.message || error);
  } finally {
    $("vaultSave").disabled = false;
  }
}

/// A `vault` frame: a new revision exists. Read it again (the frame carries
/// no text).
function onVaultFrame(frame) {
  if (frame.room !== state.room) return;
  if (state.tab === "vault") fetchVault();
  else vaultDoc = null;
}

// "Edit" beside the title: the editor is always open below the rules for
// those who may write them; the button only brings it into view.
$("vaultEditJump").addEventListener("click", () => {
  const text = $("vaultText");
  if (typeof text.scrollIntoView === "function") text.scrollIntoView({ block: "center", behavior: "smooth" });
  text.focus();
});
