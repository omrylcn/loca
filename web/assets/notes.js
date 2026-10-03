"use strict";
const noteDrafts = new Map();
const noteSaves = new Set();
function noteDraftKey(room, key) { return JSON.stringify([room, key]); }
function captureNoteDraft() {
  const editor = $("ed-body");
  if (!editor) return;
  noteDrafts.set(noteDraftKey(editor.dataset.room, editor.dataset.key), {
    title: $("ed-title").value, body: $("ed-body").value,
    write: $("ed-write")?.value,
  });
}
function cancelNoteEdit() {
  noteDrafts.delete(noteDraftKey(state.room, state.editing));
  state.editing = null;
  renderNotes();
}
function noteWriteError(error) {
  $("notesError").textContent = String(error.message || error);
  $("notesError").classList.remove("hidden");
}
// Durable shared notes and note-frame updates. Notes are not loca memory.
function onNoteFrame(note) {
  const isNew = !state.notes[note.key];
  state.notes[note.key] = note;
  if (state.tab === "notes") renderNotes(note.key);
  else { $("notesDot").classList.add("on"); }  // unseen change indicator
}

/// The lobby: building members with no active loca invitation. It lives beside
/// the roster but is not a room — there is no chat, history or task surface.
async function fetchNotes() {
  if (!state.room) return;
  const room = state.room;
  const context = state.locaContext;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/notes`, { headers: adminHeaders({}) });
    if (!r.ok) throw new Error(await r.text() || `HTTP ${r.status}`);
    const list = await r.json();
    if (state.room !== room || state.locaContext !== context) return;
    state.notes = {};
    for (const n of list) state.notes[n.key] = n;
    if (state.tab === "notes") renderNotes();
  } catch (e) { if (state.room === room && state.locaContext === context) noteWriteError(e); }
}

// Operator authority (can_write assignment) tracks admin authority: the
// server derives it from the admin token, not from the username or body.
function isOperator() { return isAdmin(); }

function renderNotes(flashKey) {
  captureNoteDraft();
  const box = $("noteList");
  const keys = Object.keys(state.notes).sort();
  if (!keys.length) { box.innerHTML = `<div class="sysline">no notes yet — create one below</div>`; return; }
  box.innerHTML = "";
  for (const key of keys) {
    const n = state.notes[key];
    const editing = state.editing === key;
    const el = document.createElement("div");
    el.className = "note" + (key === flashKey ? " flash" : "");
    const when = new Date(n.updated_at).toLocaleTimeString();
    const cw = n.can_write && n.can_write.length ? `assigned: <b>${n.can_write.map(esc).join(", ")}</b>` : "anyone may write";
    if (editing) {
      const draft = noteDrafts.get(noteDraftKey(state.room, key));
      el.innerHTML = `
        <div class="nhead"><span class="nkey">${esc(key)}</span></div>
        <input class="nedit" id="ed-title" value="${esc(draft?.title ?? n.title)}" />
        <textarea id="ed-body" data-room="${esc(state.room)}" data-key="${esc(key)}">${esc(draft?.body ?? n.body)}</textarea>
        ${isOperator() ? `<input class="nedit" id="ed-write" value="${esc(draft?.write ?? (n.can_write||[]).join(", "))}" placeholder="can_write (comma), blank = anyone" />` : ""}
        <div class="nactions">
          <button data-save="${esc(key)}">Save</button>
          <button data-cancel="1">Cancel</button>
        </div>`;
    } else {
      el.innerHTML = `
        <div class="nhead">
          <span class="nkey">${esc(key)}</span>
          <span class="ntitle">${esc(n.title)}</span>
          <span class="nmeta">rev ${n.rev} · ${esc(n.updated_by)} · ${when}</span>
        </div>
        <div class="nbody markdown">${renderMarkdown(n.body)}</div>
        <div class="nwrite">${cw}</div>
        <div class="nactions"><button data-edit="${esc(key)}">Edit</button><button data-hist="${esc(key)}">History</button><button data-del="${esc(key)}">Delete</button></div>
        <div class="nhist hidden" id="nhist-${esc(key)}"></div>`;
    }
    box.appendChild(el);
  }
}

async function createNote() {
  const key = $("nnKey").value.trim();
  if (!key || !state.room) return;
  const can_write = $("nnWrite").value.split(",").map(s => s.trim()).filter(Boolean);
  const body = {
    key, title: $("nnTitle").value.trim() || key, body: $("nnBody").value,
    by: state.name, by_type: "user", can_write,
  };
  const room = state.room;
  const context = state.locaContext;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/notes`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
    });
    if (state.room !== room || state.locaContext !== context) return;
    if (!r.ok) throw new Error(await r.text() || `HTTP ${r.status}`);
    $("notesError").classList.add("hidden");
    if ($("nnKey").value.trim() === key && $("nnBody").value === body.body
        && ($("nnTitle").value.trim() || key) === body.title
        && JSON.stringify($("nnWrite").value.split(",").map(s => s.trim()).filter(Boolean)) === JSON.stringify(can_write)) {
      $("nnKey").value = $("nnTitle").value = $("nnBody").value = $("nnWrite").value = "";
    }
  } catch (error) { if (state.room === room && state.locaContext === context) noteWriteError(error); }
  // live "note" frame will refresh the list.
}

async function saveNote(key) {
  const room = state.room;
  const context = state.locaContext;
  const operation = noteDraftKey(room, key);
  if (noteSaves.has(operation)) return;
  captureNoteDraft();
  const body = { by: state.name, by_type: "user" };
  body.title = $("ed-title").value;
  body.body = $("ed-body").value;
  if (isOperator() && $("ed-write")) {
    body.can_write = $("ed-write").value.split(",").map(s => s.trim()).filter(Boolean);
  }
  // Operator authority is carried by the x-admin-token header, not the body.
  noteSaves.add(operation);
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/notes/${encodeURIComponent(key)}`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
    });
    if (state.room !== room || state.locaContext !== context) return;
    if (!r.ok) throw new Error(await r.text() || `HTTP ${r.status}`);
    $("notesError").classList.add("hidden");
    if (state.editing === key && $("ed-body")?.value === body.body && $("ed-title")?.value === body.title
        && (!body.can_write || JSON.stringify($("ed-write").value.split(",").map(s => s.trim()).filter(Boolean)) === JSON.stringify(body.can_write))) {
      noteDrafts.delete(operation);
      state.editing = null;
      renderNotes();
    }
  } catch (error) { if (state.room === room && state.locaContext === context) noteWriteError(error); }
  finally { noteSaves.delete(operation); }
}

/* ---- restart-epoch: detect a server restart and resync ---- */
