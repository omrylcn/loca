"use strict";
// Durable shared notes and note-frame updates.
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
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/notes`, { headers: adminHeaders({}) });
    const list = await r.json();
    state.notes = {};
    for (const n of list) state.notes[n.key] = n;
    if (state.tab === "notes") renderNotes();
  } catch (e) {}
}

// Operator authority (can_write assignment) tracks admin authority: the
// server derives it from the admin token, not from the username or body.
function isOperator() { return isAdmin(); }

function renderNotes(flashKey) {
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
      el.innerHTML = `
        <div class="nhead"><span class="nkey">${esc(key)}</span></div>
        <input class="nedit" id="ed-title" value="${esc(n.title)}" />
        <textarea id="ed-body">${esc(n.body)}</textarea>
        ${isOperator() ? `<input class="nedit" id="ed-write" value="${esc((n.can_write||[]).join(", "))}" placeholder="can_write (comma), blank = anyone" />` : ""}
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
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/notes`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
  });
  if (r.status === 409) { alert(`note "${key}" already exists — edit it instead`); return; }
  $("nnKey").value = $("nnTitle").value = $("nnBody").value = $("nnWrite").value = "";
  // live "note" frame will refresh the list.
}

async function saveNote(key) {
  const body = { by: state.name, by_type: "user" };
  body.title = $("ed-title").value;
  body.body = $("ed-body").value;
  if (isOperator() && $("ed-write")) {
    body.can_write = $("ed-write").value.split(",").map(s => s.trim()).filter(Boolean);
  }
  // Operator authority is carried by the x-admin-token header, not the body.
  await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/notes/${encodeURIComponent(key)}`, {
    method: "PUT", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
  });
  state.editing = null;
  // live frame refreshes; render immediately for snappiness.
  renderNotes();
}

/* ---- restart-epoch: detect a server restart and resync ---- */
