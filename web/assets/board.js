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

// ---- the Loca Board (the `board` extension) ----
// Four columns over the same task list. The holder's "done" is a request for
// review; whoever gave the task (never its holder) or the Operator approves
// it or requests changes (the server's go and no-go).
// The server enforces every rule; these buttons and drops only follow them.

const BOARD_COLUMNS = [["open", "To do"], ["taken", "Doing"], ["review", "Review"], ["done", "Done"]];

// Whether this page's person holds the task: by the principal the server
// recorded, else by the name (a task given by name alone).
function holdsTask(task) {
  if (task.assigned_principal) return task.assigned_principal === state.profile?.principal?.id;
  return !!task.assigned_to && task.assigned_to === state.name;
}

// Whose review a task waits for (the server's `task_reviewer`): who gave it
// to its holder, or, when the holder took it, who declared it. Never the
// holder.
function taskReviewerName(task) {
  const holder = task.assigned_to || null;
  const by = task.assigned_by && task.assigned_by !== holder ? task.assigned_by : task.created_by;
  return by && by !== holder ? by : null;
}

// What this page's person may do with a task: each move's button, and the
// column a card may be dropped on for it (none for Remove).
function boardMoves(task) {
  const mine = holdsTask(task);
  const operator = isLocaOperator();
  const reviewer = operator || (!mine && taskReviewerName(task) === state.name);
  const moves = [];
  if (task.status === "open" && (mine || !task.assigned_to)) moves.push({ act: "take", to: "taken", label: "Start" });
  if (task.status === "taken" && mine) moves.push({ act: "done", to: "review", label: "Ask for review" });
  if (task.status === "review" && reviewer) {
    moves.push({ act: "go", to: "done", label: "Approve" });
    moves.push({ act: "nogo", to: "taken", label: "Request changes" });
  }
  if (operator) {
    if (["open", "taken", "review"].includes(task.status)) moves.push({ act: "cancel", to: null, label: "Remove" });
    if (task.status === "done" || task.status === "cancelled") moves.push({ act: "reopen", to: "open", label: "Reopen" });
  }
  return moves;
}

function boardActions(task) {
  return boardMoves(task)
    .map(m => `<button data-tact="${m.act}" data-tid="${task.id}">${m.label}</button>`).join("");
}

function boardCard(task) {
  const el = document.createElement("div");
  el.className = "task";
  el.dataset.tid = String(task.id);
  // A card that can move is picked up and dropped on a column (boardmove.js).
  if (boardMoves(task).some(m => m.to)) el.draggable = true;
  const who = task.assigned_to ? ` · ${esc(task.assigned_to)}` : "";
  const note = task.status === "review" && task.review_note ? `<span class="tmeta">Note: ${esc(task.review_note)}</span>` : "";
  const reason = task.status === "taken" && task.review_reason ? `<span class="tmeta">Not yet: ${esc(task.review_reason)}</span>` : "";
  el.innerHTML = `<span class="tid">#${task.id}</span>
    <span class="ttitle">${esc(task.title)}
      <span class="tmeta">Added by ${esc(task.created_by)}${who}</span>${note}${reason}
    </span>
    <span class="tchip ${esc(task.status)}">${esc(TLABEL[task.status] || task.status)}</span>
    <span class="tacts">${boardActions(task)}</span>${typeof taskFormFor === "function" ? taskFormFor(task) : ""}`;
  return el;
}

// `tasks`: what the filters let through (boardlist.js); `onlyStage`: on a
// phone, the one column shown, with a switcher between the four.
function renderBoard(box, tasks = Object.values(state.tasks), onlyStage = null) {
  tasks = [...tasks].sort((a, b) => b.id - a.id);
  const inColumn = status => tasks.filter(t => t.status === status || (status === "done" && t.status === "cancelled"));
  const board = document.createElement("div");
  board.className = onlyStage ? "taskboard onestage" : "taskboard";
  board.setAttribute("aria-label", "Loca Board");
  box.innerHTML = "";
  if (onlyStage) {
    const switcher = document.createElement("div");
    switcher.className = "boardstages";
    switcher.setAttribute("role", "radiogroup");
    switcher.setAttribute("aria-label", "Stage");
    switcher.innerHTML = BOARD_COLUMNS.map(([status, label]) =>
      `<button type="button" role="radio" aria-checked="${status === onlyStage}" class="${status === onlyStage ? "selected" : ""}" ` +
      `data-board-stage="${status}">${label} · ${inColumn(status).length}</button>`).join("");
    box.appendChild(switcher);
  }
  for (const [status, label] of BOARD_COLUMNS) {
    if (onlyStage && status !== onlyStage) continue;
    // Removed tasks sit with the finished ones, so the Operator can reopen them.
    const held = inColumn(status);
    const column = document.createElement("div");
    column.className = "boardcol";
    column.dataset.column = status;
    column.innerHTML = `<div class="boardhead">${label} · ${held.length}</div>`;
    for (const task of held) column.appendChild(boardCard(task));
    board.appendChild(column);
  }
  box.appendChild(board);
}

// Approve (go: review -> done) or request changes with a reason (no-go:
// review -> taken). The reason comes from the card's own form (boardmove.js).
async function boardReview(act, id, reason = null) {
  const body = { by: state.name, status: act === "go" ? "done" : "taken" };
  if (act === "nogo") {
    if (!reason) return;
    body.reason = reason;
  }
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/tasks/${id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
  });
  if (!r.ok) addSys("Could not review the task: " + await r.text());
}

// ---- the Loca Board's tab, and Focus's line about it ----

// The tab: the four columns, and the declare form for those who may.
function renderBoardTab() {
  $("taskCreate").classList.toggle("hidden", !mayDeclareTasks());
  renderBoardViews($("taskList"));
}

// One line in the Focus tab. With the board on: its live counts, and the way
// to it. With it off and tasks stored: where they went, and for the
// operator the way to bring them back (the board needs a Lead).
function renderFocusBoardLine() {
  const line = $("focusBoardLine");
  const room = state.room;
  if (!room || isDm(room) || currentExtensions() === null) { line.classList.add("hidden"); return; }
  if (extensionOn("board")) {
    const tasks = Object.values(state.tasks || {}).filter(t => t.room === room);
    const parts = [["open", "to do"], ["taken", "doing"], ["review", "in review"]]
      .map(([status, word]) => [tasks.filter(t => t.status === status).length, word])
      .filter(([n]) => n > 0)
      .map(([n, word]) => `${n} ${word}`);
    const goal = currentGoal();
    let left = [];
    if (goal?.completion === "all_tasks") {
      left = (goal.task_ids || []).filter(id => state.tasks[id]?.status !== "done");
      if (left.length) parts.push(`${left.length} left before the goal closes`);
    }
    // The tasks the goal still waits for open their own view (taskview.js).
    const links = left.map(id => `<a href="#" data-task-ref="${Number(id)}">#${Number(id)}</a>`).join(" ");
    line.innerHTML = `<span><b>Loca Board:</b> ${esc(parts.length ? parts.join(" · ") : "nothing open")}${links ? ` ${links}` : ""}</span>` +
      `<button type="button" data-open-board>Open Loca Board</button>`;
    line.classList.remove("hidden");
    return;
  }
  if (!Number(state.locaExtensions?.stored_tasks || 0)) { line.classList.add("hidden"); return; }
  const action = !isLocaOperator() ? ""
    : state.settings?.lead ? `<button type="button" data-board-on>Turn on Loca Board</button>`
    : `<small>Name a Lead to turn it on.</small>`;
  line.innerHTML = `<span>Tasks now live in the Loca Board tab, which needs a Lead.</span>${action}`;
  line.classList.remove("hidden");
}

// The board came or went (applyLocaExtensions): its tasks are fetched or
// dropped, and the loca's view says again how many are stored.
function boardSetChanged() {
  fetchTasks();
  fetchLocaExtensions();
}

async function turnOnBoard() {
  const room = state.room;
  const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/extensions/board`, {
    method: "PUT", headers: adminHeaders({}),
  });
  if (!response.ok) { addSys("Could not turn on the Loca Board: " + await response.text()); return; }
  if (room !== state.room) return;
  const view = await response.json();
  state.locaExtensions = view;
  state.settings = { ...(state.settings || {}), extensions: view.enabled };
  renderSettings();
}

$("focusBoardLine").addEventListener("click", (e) => {
  if (e.target.closest("[data-open-board]")) switchTab("board");
  else if (e.target.closest("[data-board-on]")) turnOnBoard();
});

// ---- turning the board off while tasks are not done ----
// The server answers 409 with the counts (`unfinished`); the Manage list
// asks what becomes of them, keep being the default, and sends the answer
// as `?open_tasks=` (extensions.js changeExtension).

/// Show the question for a 409 body; false when it is not that question.
function askBoardOff(said) {
  let body = null;
  try { body = JSON.parse(said); } catch (e) { return false; }
  const u = body?.unfinished;
  if (!u) return false;
  const total = u.open + u.taken + u.review;
  const box = $("boardOffAsk");
  box.innerHTML = `<p class="extnote"><b>${total} ${total === 1 ? "task is" : "tasks are"} not done</b> on the Loca Board: ` +
    `${u.open} to do · ${u.taken} doing · ${u.review} in review. What becomes of ${total === 1 ? "it" : "them"}?</p>` +
    `<p class="extnote">Kept tasks are not shown while the board is off, and come back as they are when it is turned on again.</p>` +
    `<div class="extactions" style="flex-wrap: wrap">` +
    `<button type="button" class="btn" data-board-off="">Cancel</button>` +
    `<button type="button" class="btn" data-board-off="remove">Remove them</button>` +
    `<button type="button" class="btn" data-board-off="done">Mark them done</button>` +
    `<button type="button" class="btn primary" data-board-off="keep">Keep them</button></div>`;
  box.classList.remove("hidden");
  box.querySelector('[data-board-off="keep"]').focus();
  return true;
}

function hideBoardOffAsk() {
  $("boardOffAsk").classList.add("hidden");
  $("boardOffAsk").innerHTML = "";
}

$("boardOffAsk").addEventListener("click", (e) => {
  const button = e.target.closest("[data-board-off]");
  if (!button) return;
  const choice = button.dataset.boardOff;
  hideBoardOffAsk();
  if (choice) changeExtension("board", false, choice);
  else renderExtensionManage();
});
for (const id of ["extManageClose", "extManageBackdrop"]) $(id).addEventListener("click", hideBoardOffAsk);
