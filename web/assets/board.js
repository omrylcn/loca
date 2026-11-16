"use strict";
// The Lead's task controls. The server decides who may declare and assign
// (the Lead is checked by principal there); the page only stops offering
// what the server would refuse.

// Whether the person at this page holds this loca's Lead title: the
// principal the server stored with the title, or, where none was stored
// (the open sandbox), the name.
function isLocaLead() {
  const settings = state.settings;
  if (!settings?.lead || !state.room || isDm(state.room)) return false;
  if (settings.lead_principal) return settings.lead_principal === state.profile?.principal?.id;
  return settings.lead === state.name;
}

// Who may declare a task here, and give one to someone.
function mayDeclareTasks() {
  return isLocaOperator() || isLocaLead();
}

// ---- the Task board (the `board` extension) ----
// Four columns over the same task list. The holder's "done" is a request for
// review; the Lead (never on its own task) or the Operator says go or no-go.
// The server enforces every rule; these buttons only follow them.

const BOARD_COLUMNS = [["open", "To do"], ["taken", "Doing"], ["review", "Review"], ["done", "Done"]];

// Whether this page's person holds the task: by the principal the server
// recorded, else by the name (a task given by name alone).
function holdsTask(task) {
  if (task.assigned_principal) return task.assigned_principal === state.profile?.principal?.id;
  return !!task.assigned_to && task.assigned_to === state.name;
}

function boardActions(task) {
  const mine = holdsTask(task);
  const reviewer = isLocaOperator() || (isLocaLead() && !mine);
  let acts = "";
  if (task.status === "open" && (mine || !task.assigned_to)) acts += `<button data-tact="take" data-tid="${task.id}">Start</button>`;
  if (task.status === "taken" && mine) acts += `<button data-tact="done" data-tid="${task.id}">Ask for review</button>`;
  if (task.status === "review" && reviewer) {
    acts += `<button data-tact="go" data-tid="${task.id}">Go</button>`;
    acts += `<button data-tact="nogo" data-tid="${task.id}">No-go</button>`;
  }
  if (isLocaOperator()) {
    if (["open", "taken", "review"].includes(task.status)) acts += `<button data-tact="cancel" data-tid="${task.id}">Remove</button>`;
    if (task.status === "done" || task.status === "cancelled") acts += `<button data-tact="reopen" data-tid="${task.id}">Reopen</button>`;
  }
  return acts;
}

function boardCard(task) {
  const el = document.createElement("div");
  el.className = "task";
  el.dataset.tid = String(task.id);
  const who = task.assigned_to ? ` · ${esc(task.assigned_to)}` : "";
  const note = task.status === "review" && task.review_note ? `<span class="tmeta">Note: ${esc(task.review_note)}</span>` : "";
  const reason = task.status === "taken" && task.review_reason ? `<span class="tmeta">Not yet: ${esc(task.review_reason)}</span>` : "";
  el.innerHTML = `<span class="tid">#${task.id}</span>
    <span class="ttitle">${esc(task.title)}
      <span class="tmeta">Added by ${esc(task.created_by)}${who}</span>${note}${reason}
    </span>
    <span class="tchip ${esc(task.status)}">${esc(TLABEL[task.status] || task.status)}</span>
    <span class="tacts">${boardActions(task)}</span>`;
  return el;
}

function renderBoard(box) {
  const tasks = Object.values(state.tasks).sort((a, b) => b.id - a.id);
  const board = document.createElement("div");
  board.className = "taskboard";
  board.setAttribute("aria-label", "Task board");
  for (const [status, label] of BOARD_COLUMNS) {
    // Removed tasks sit with the finished ones, so the Operator can reopen them.
    const held = tasks.filter(t => t.status === status || (status === "done" && t.status === "cancelled"));
    const column = document.createElement("div");
    column.className = "boardcol";
    column.dataset.column = status;
    column.innerHTML = `<div class="boardhead">${label} · ${held.length}</div>`;
    for (const task of held) column.appendChild(boardCard(task));
    board.appendChild(column);
  }
  box.innerHTML = "";
  box.appendChild(board);
}

// Go (review -> done) or no-go with a reason (review -> taken).
async function boardReview(act, id) {
  const body = { by: state.name, status: act === "go" ? "done" : "taken" };
  if (act === "nogo") {
    const reason = (window.prompt(`Why is task #${id} not done yet? The holder reads this.`) || "").trim();
    if (!reason) return;
    body.reason = reason;
  }
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/tasks/${id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
  });
  if (!r.ok) addSys("Could not review the task: " + await r.text());
}
