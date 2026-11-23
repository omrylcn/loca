"use strict";
// Each task is an object with a view of its own: a dialog showing its
// number, title, stage, holder, who declared and who assigned it, its
// timestamps and source message, and only the actions this viewer may take.
// It has a stable address, `#board/<loca>/<n>`, and opens from a card or a
// row, a "task #N" in chat, a roster badge or the Focus line. The server
// decides every action (the existing task routes); the view only offers
// what it would accept.

let taskViewOpen = null; // { room, id } while the dialog shows

function taskAddress(room, id) {
  return `#board/${encodeURIComponent(room)}/${id}`;
}

// The page's address after `#`. The view only reads it and replaces the
// current history entry (never pushes one), so Back never lands on a page
// state nobody chose; see test_web_auth_contract.py.
function pageAddress() {
  return new URL(window.location.href).hash;
}

/// "task #N" in already escaped chat text, as a link to that task here.
function taskRefLinks(html) {
  if (!state.room || isDm(state.room) || !extensionOn("board")) return html;
  return html.replace(/\b([Tt]ask) #(\d{1,9})\b/g, (all, word, id) =>
    `<a href="${taskAddress(state.room, id)}" class="taskref" data-task-ref="${id}">${word} #${id}</a>`);
}

function taskWho(name, as) {
  if (!name) return "—";
  return as === "lead" ? `Lead ${name}` : name;
}

// Whether this viewer may give the task to someone: an operator on any
// unfinished task, the Lead within its bounds (the server's lead_may_assign).
function mayReassign(task) {
  if (isLocaOperator()) return ["open", "taken", "review"].includes(task.status);
  return isLocaLead() && ["open", "taken"].includes(task.status)
    && (!task.assigned_to || ["lead", "participant"].includes(task.assigned_as));
}

// The review history the server keeps (`task.reviews`), oldest first, read
// only: who asked for review and with what note, who said go, who said no-go
// and why.
function taskHistoryHtml(task) {
  const reviews = task.reviews || [];
  if (!reviews.length) return "";
  const said = { asked: "asked for review", go: ": go", nogo: ": not done yet" };
  const items = reviews.map(review => {
    const at = Number(review.at || 0);
    const verb = said[review.kind] || review.kind;
    const what = verb.startsWith(":") ? `${esc(review.by)}${esc(verb)}` : `${esc(review.by)} ${esc(verb)}`;
    const text = review.text ? `<span class="taskhistorytext">${esc(review.text)}</span>` : "";
    return `<li><span>${what}</span> <time datetime="${new Date(at).toISOString()}">${esc(fmtFull(at))}</time>${text}</li>`;
  });
  return `<div class="sectionlabel">Review history</div><ol class="taskhistory">${items.join("")}</ol>`;
}

function renderTaskView(task) {
  const time = ms => ms ? `<time datetime="${new Date(ms).toISOString()}">${esc(fmtFull(ms))}</time>` : "—";
  const rows = [
    ["Stage", `<span class="tchip ${esc(task.status)}">${esc(TLABEL[task.status] || task.status)}</span>`],
    ["Holder", esc(task.assigned_to || "nobody")],
    ["Declared by", esc(taskWho(task.created_by, task.created_as))],
    ["Assigned by", esc(task.assigned_to ? taskWho(task.assigned_by, task.assigned_as) : "—")],
    ["Declared", time(task.created_at)],
    ["Last change", time(task.progress_at)],
  ];
  if (task.closed_at) rows.push(["Closed", time(task.closed_at)]);
  if (task.from_message) rows.push(["Source", `<a href="#" data-goto="${Number(task.from_message)}">the message it grew from</a>`]);
  if (task.status === "review" && task.review_note) rows.push(["Note for review", noteWithLinks(task.review_note)]);
  const delivered = deliveredNote(task);
  if (delivered) rows.push(["Delivered", noteWithLinks(delivered)]);
  if (task.status === "taken" && task.review_reason) rows.push(["Not done yet", esc(task.review_reason)]);
  const reassign = mayReassign(task)
    ? `<form class="taskreassign" id="taskReassign"><label>Give it to <input id="taskReassignName" autocomplete="off" placeholder="name"></label>` +
      `<button type="submit" class="btn">Give</button></form>`
    : "";
  $("taskViewTitle").textContent = `#${task.id} ${task.title}`;
  $("taskViewBody").innerHTML =
    `<dl class="taskfacts">${rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>` +
    `<div class="tacts taskviewacts">${boardActions(task)}</div>${reassign}${taskHistoryHtml(task)}`;
}

async function fetchTaskFor(room, id) {
  const known = room === state.room ? state.tasks[id] : null;
  if (known) return known;
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/tasks/${id}`, { headers: adminHeaders({}) });
  if (r.status === 404) throw new Error(`There is no task #${id} in this loca.`);
  if (!r.ok) throw new Error(await r.text() || `Could not open task #${id}.`);
  return r.json();
}

/// Open task `id` of the loca on screen, and give the page its address.
async function openTaskView(id, room = state.room) {
  if (!room) return;
  try {
    const task = await fetchTaskFor(room, id);
    taskViewOpen = { room, id };
    $("taskViewCopied").textContent = "";
    renderTaskView(task);
    if (pageAddress() !== taskAddress(room, id)) history.replaceState(null, "", taskAddress(room, id));
    $("taskView").classList.remove("hidden");
    setTimeout(() => $("taskViewClose").focus(), 0);
  } catch (error) {
    addSys(error.message || "Could not open the task.");
  }
}

function closeTaskView() {
  taskViewOpen = null;
  $("taskView").classList.add("hidden");
  if (pageAddress().startsWith("#board/")) history.replaceState(null, "", location.pathname + location.search);
}

/// A task frame: redraw an open view of that task.
function refreshTaskView(task) {
  if (taskViewOpen && taskViewOpen.room === task.room && taskViewOpen.id === task.id) renderTaskView(task);
}

// `#board/<loca>/<n>` (or `#board/<n>` for the loca on screen): enter the
// loca if needed, show its Loca Board, open the task.
async function openTaskFromAddress() {
  const match = pageAddress().match(/^#board\/(?:([^/]+)\/)?(\d{1,9})$/);
  if (!match || state.authStatus !== "authenticated") return;
  const room = match[1] ? decodeURIComponent(match[1]) : state.room;
  const id = Number(match[2]);
  if (!room || (taskViewOpen && taskViewOpen.room === room && taskViewOpen.id === id)) return;
  if (room !== state.room) joinRoom(room);
  for (let i = 0; i < 100 && !(state.room === room && isLocaContextReady("settings") && extensionCatalogList()); i++) {
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (state.room !== room) return;
  if (extensionOn("board")) switchTab("board");
  openTaskView(id, room);
}

$("taskViewClose").addEventListener("click", closeTaskView);
$("taskViewBackdrop").addEventListener("click", closeTaskView);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && taskViewOpen) closeTaskView();
});
$("taskViewCopy").addEventListener("click", async () => {
  if (!taskViewOpen) return;
  const link = `${location.origin}${location.pathname}${taskAddress(taskViewOpen.room, taskViewOpen.id)}`;
  const ok = await copyText(link, $("taskViewTitle"));
  $("taskViewCopied").textContent = ok ? "Link copied." : `Copy failed; the link is ${link}`;
});
$("taskViewBody").addEventListener("click", (e) => {
  const g = e.target.closest("[data-goto]");
  if (g) { e.preventDefault(); closeTaskView(); switchTab("chat"); gotoMsg(g.dataset.goto); return; }
  const b = e.target.closest("[data-tact]");
  if (b) taskAct(b.dataset.tact, Number(b.dataset.tid), b);
});
$("taskViewBody").addEventListener("submit", async (e) => {
  if (e.target.id !== "taskReassign" || !taskViewOpen) return;
  e.preventDefault();
  const name = $("taskReassignName").value.trim();
  if (!name) return;
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(taskViewOpen.room)}/tasks/${taskViewOpen.id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ assigned_to: name, by: state.name }),
  });
  if (!r.ok) addSys("Could not give the task: " + await r.text());
});
// "task #N" in chat, a roster badge, Focus's line, or a card or row's title.
document.addEventListener("click", (e) => {
  const ref = e.target.closest("[data-task-ref]");
  if (ref) { e.preventDefault(); openTaskView(Number(ref.dataset.taskRef)); return; }
  if (e.target.closest("button, a, input, select, label, form")) return;
  const card = e.target.closest("#taskList .task[data-tid]");
  if (card) openTaskView(Number(card.dataset.tid));
});
window.addEventListener("hashchange", openTaskFromAddress);
// A page opened at a task's address: once signed in, go there.
(async () => {
  if (!pageAddress().startsWith("#board/")) return;
  for (let i = 0; i < 300 && state.authStatus !== "authenticated"; i++) {
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  openTaskFromAddress();
})();
