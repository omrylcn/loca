"use strict";
// Moving a task on the Loca Board. Two things, both over board.js's
// boardMoves (the server's rules as this page knows them):
// - the small form a review move asks for, on the card itself: an optional
//   note when asking for review, a reason for a no-go;
// - dragging a card onto the column of a move its person may make. While a
//   card is held, those columns open and the others go quiet; a drop does
//   exactly what that move's button does. The buttons stay, for the keyboard
//   and for phones.

// The form open on a card: { act, id, draft }. Kept so a redraw of the board
// (another task changed) does not lose what was being written.
let taskForm = null;

const TASK_FORM = {
  done: { label: "A note for the reviewer (optional)", placeholder: "what changed, and how to check it", send: "Send for review", required: false },
  nogo: { label: "What should change? The holder reads this.", placeholder: "the reason", send: "Send back", required: true },
};

function taskFormHtml(act, id, draft = "") {
  const f = TASK_FORM[act];
  return `<form class="tform" data-tform="${act}" data-tid="${id}">`
    + `<label>${esc(f.label)}<textarea rows="2" maxlength="2000" placeholder="${esc(f.placeholder)}"${f.required ? " required" : ""}>${esc(draft)}</textarea></label>`
    + `<span class="tformacts"><button type="submit">${esc(f.send)}</button>`
    + `<button type="button" data-tformcancel>Cancel</button></span></form>`;
}

// The form for a card drawn again: only while that move is still its
// person's to make.
function taskFormFor(task) {
  if (!taskForm || taskForm.id !== task.id) return "";
  if (!boardMoves(task).some(m => m.act === taskForm.act)) { taskForm = null; return ""; }
  return taskFormHtml(taskForm.act, task.id, taskForm.draft);
}

function openTaskForm(act, id, from) {
  document.querySelectorAll(".tform").forEach(f => f.remove());
  const host = from?.closest(".task, .taskviewacts")
    || document.querySelector(`#taskList .task[data-tid="${id}"]`);
  if (!host || !TASK_FORM[act]) { taskForm = null; return; }
  taskForm = { act, id, draft: "" };
  host.insertAdjacentHTML(host.classList.contains("task") ? "beforeend" : "afterend", taskFormHtml(act, id));
  document.querySelector(`.tform[data-tid="${id}"] textarea`)?.focus();
}

document.addEventListener("input", (e) => {
  const form = e.target.closest?.(".tform");
  if (form && taskForm && Number(form.dataset.tid) === taskForm.id) taskForm.draft = e.target.value;
});
document.addEventListener("click", (e) => {
  const cancel = e.target.closest?.("[data-tformcancel]");
  if (!cancel) return;
  taskForm = null;
  cancel.closest(".tform")?.remove();
});
document.addEventListener("submit", async (e) => {
  const form = e.target.closest?.(".tform");
  if (!form) return;
  e.preventDefault();
  const act = form.dataset.tform;
  const text = form.querySelector("textarea").value.trim();
  if (TASK_FORM[act]?.required && !text) return;
  taskForm = null;
  form.remove();
  await taskAct(act, Number(form.dataset.tid), null, text);
});

// ---- drag and drop ----

let boardDrag = null; // { id, moves, card }

function boardDropColumns() {
  return [...document.querySelectorAll("#taskList .taskboard .boardcol")];
}

function endBoardDrag() {
  for (const col of boardDropColumns()) {
    col.classList.remove("dropok", "dropno", "dropover");
    col.removeAttribute("title");
  }
  boardDrag?.card?.classList.remove("dragging");
  boardDrag = null;
}

document.addEventListener("dragstart", (e) => {
  const card = e.target.closest?.("#taskList .taskboard .task[draggable='true']");
  if (!card) return;
  const task = state.tasks[Number(card.dataset.tid)];
  if (!task) return;
  const moves = boardMoves(task).filter(m => m.to);
  boardDrag = { id: task.id, moves, card };
  e.dataTransfer.effectAllowed = "move";
  e.dataTransfer.setData("text/plain", `#${task.id}`);
  card.classList.add("dragging");
  const home = card.closest(".boardcol");
  for (const col of boardDropColumns()) {
    if (col === home) continue;
    const ok = moves.some(m => m.to === col.dataset.column);
    col.classList.add(ok ? "dropok" : "dropno");
    if (!ok) col.title = "This task cannot move here for you";
  }
});

document.addEventListener("dragover", (e) => {
  const col = boardDrag && e.target.closest?.("#taskList .boardcol.dropok");
  if (!col) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = "move";
  col.classList.add("dropover");
});

document.addEventListener("dragleave", (e) => {
  const col = e.target.closest?.("#taskList .boardcol.dropover");
  if (col && !col.contains(e.relatedTarget)) col.classList.remove("dropover");
});

document.addEventListener("drop", (e) => {
  const col = boardDrag && e.target.closest?.("#taskList .boardcol.dropok");
  if (!col) return;
  e.preventDefault();
  const { id, moves, card } = boardDrag;
  const move = moves.find(m => m.to === col.dataset.column);
  endBoardDrag();
  if (move) taskAct(move.act, id, card);
});

document.addEventListener("dragend", endBoardDrag);
