"use strict";
// The Loca Board's two views of the same tasks: a List (rows, filtered and
// sorted) and the Board (four columns, board.js). Mine, holder and stage
// narrow either; the List sorts by last change (the default), number or
// stage. The choice of view is this browser's alone (localStorage); on a
// phone the List comes first, and the Board shows one stage at a time.

const BOARD_VIEW_KEY = "loca-board-view";
const BOARD_STAGES = [["open", "To do"], ["taken", "Doing"], ["review", "Review"], ["done", "Done"], ["cancelled", "Removed"]];
const BOARD_STAGE_ORDER = { open: 0, taken: 1, review: 2, done: 3, cancelled: 4 };
// Filters and sort live for this page only; the view is remembered.
let boardFilter = { mine: false, holder: "", stage: "", sort: "updated" };
let boardPhoneStage = "open";

function boardNarrow() {
  return typeof window.matchMedia === "function" && window.matchMedia("(max-width: 760px)").matches;
}

/// "list" or "board": what this browser chose, else the List on a phone.
function boardView() {
  try {
    const saved = localStorage.getItem(BOARD_VIEW_KEY);
    if (saved === "list" || saved === "board") return saved;
  } catch (e) { /* storage unavailable: fall back to the default */ }
  return boardNarrow() ? "list" : "board";
}

function setBoardView(view) {
  try { localStorage.setItem(BOARD_VIEW_KEY, view); } catch (e) { /* kept for this page only */ }
  boardViewNow = view;
  renderBoardTab();
}
// The view in use when storage cannot keep it.
let boardViewNow = null;

/// The tasks the filters let through (stage only in the List).
function boardTasksFiltered(withStage) {
  return Object.values(state.tasks || {}).filter(task => task.room === state.room)
    .filter(task => !boardFilter.mine || holdsTask(task))
    .filter(task => !boardFilter.holder || task.assigned_to === boardFilter.holder)
    .filter(task => {
      if (!withStage) return true;
      // "Every stage but Removed" unless one is chosen.
      return boardFilter.stage ? task.status === boardFilter.stage : task.status !== "cancelled";
    });
}

function boardSorted(tasks) {
  const by = {
    updated: (a, b) => Number(b.progress_at || b.created_at || 0) - Number(a.progress_at || a.created_at || 0) || b.id - a.id,
    number: (a, b) => a.id - b.id,
    stage: (a, b) => (BOARD_STAGE_ORDER[a.status] ?? 9) - (BOARD_STAGE_ORDER[b.status] ?? 9) || b.id - a.id,
  }[boardFilter.sort] || ((a, b) => b.id - a.id);
  return [...tasks].sort(by);
}

function boardAgo(ms) {
  const seconds = Math.max(0, Math.round((Date.now() - Number(ms || 0)) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} d ago`;
}

function boardOption(value, label, chosen) {
  return `<option value="${esc(value)}"${value === chosen ? " selected" : ""}>${esc(label)}</option>`;
}

// Who can hold a task here: everyone with a seat at this table, and anyone
// who already holds one. The new task's owner and the Holder filter list the
// same people.
function boardPeople() {
  const seated = (state.members || []).map(m => m.name).filter(Boolean);
  const holding = Object.values(state.tasks || {})
    .filter(task => task.room === state.room && task.assigned_to).map(task => task.assigned_to);
  return [...new Set([...seated, ...holding])].sort((a, b) => a.localeCompare(b));
}

// The new task's owner: a pick from the table, nobody by default.
function renderTaskOwnerChoices() {
  const pick = $("tkAssign");
  if (!pick) return;
  const chosen = pick.value;
  const people = boardPeople();
  pick.innerHTML = boardOption("", "Who owns it? (optional)", chosen)
    + people.map(name => boardOption(name, name, chosen)).join("");
  pick.value = people.includes(chosen) ? chosen : "";
}
$("taskCreate")?.addEventListener("toggle", () => { if ($("taskCreate").open) renderTaskOwnerChoices(); });

function renderBoardTools(view) {
  const holders = boardPeople();
  renderTaskOwnerChoices();
  if (boardFilter.holder && !holders.includes(boardFilter.holder)) holders.push(boardFilter.holder);
  const tab = (value, label) => `<button type="button" role="radio" aria-checked="${view === value}" ` +
    `class="${view === value ? "selected" : ""}" data-board-view="${value}">${label}</button>`;
  const stage = view === "list"
    ? `<label>Stage <select id="boardStage">${boardOption("", "Every stage but Removed", boardFilter.stage)}` +
      BOARD_STAGES.map(([value, label]) => boardOption(value, label, boardFilter.stage)).join("") + `</select></label>` +
      `<label>Sort <select id="boardSort">${boardOption("updated", "Last change", boardFilter.sort)}` +
      `${boardOption("number", "Number", boardFilter.sort)}${boardOption("stage", "Stage", boardFilter.sort)}</select></label>`
    : "";
  $("boardTools").innerHTML =
    `<div class="boardviewswitch" role="radiogroup" aria-label="Loca Board view">${tab("list", "List")}${tab("board", "Board")}</div>` +
    `<label class="boardmine"><input type="checkbox" id="boardMine"${boardFilter.mine ? " checked" : ""}> Mine</label>` +
    `<label>Holder <select id="boardHolder">${boardOption("", "Anyone", boardFilter.holder)}` +
    holders.map(name => boardOption(name, name, boardFilter.holder)).join("") + `</select></label>` + stage;
}

function renderBoardList(box) {
  const tasks = boardSorted(boardTasksFiltered(true));
  const list = document.createElement("div");
  list.className = "boardlist";
  list.setAttribute("aria-label", "Loca Board list");
  if (!tasks.length) list.innerHTML = `<div class="sysline">No task matches.</div>`;
  for (const task of tasks) {
    const row = boardCard(task);
    row.classList.add("boardrow");
    const meta = row.querySelector(".tmeta");
    if (meta) meta.textContent += ` · ${boardAgo(task.progress_at || task.created_at)}`;
    list.appendChild(row);
  }
  box.innerHTML = "";
  box.appendChild(list);
}

/// The Loca Board tab's tasks, in the chosen view.
function renderBoardViews(box) {
  const view = boardViewNow || boardView();
  renderBoardTools(view);
  if (view === "list") { renderBoardList(box); return; }
  // On a phone the Board shows one stage at a time, with a switcher.
  renderBoard(box, boardTasksFiltered(false), boardNarrow() ? boardPhoneStage : null);
}

$("boardTools").addEventListener("click", (e) => {
  const view = e.target.closest("[data-board-view]");
  if (view) setBoardView(view.dataset.boardView);
});
$("boardTools").addEventListener("change", (e) => {
  if (e.target.id === "boardMine") boardFilter.mine = e.target.checked;
  else if (e.target.id === "boardHolder") boardFilter.holder = e.target.value;
  else if (e.target.id === "boardStage") boardFilter.stage = e.target.value;
  else if (e.target.id === "boardSort") boardFilter.sort = e.target.value;
  else return;
  renderBoardTab();
});
$("taskList").addEventListener("click", (e) => {
  const stage = e.target.closest("[data-board-stage]");
  if (!stage) return;
  boardPhoneStage = stage.dataset.boardStage;
  renderBoardTab();
});
