"use strict";
// Journal, goals, tasks, waits, and bounded reminders.
async function fetchJournal() {
  if (!state.room) return;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/journal`, { headers: adminHeaders({}) });
    if (!r.ok) return;
    state.journal = await r.json();
    if (state.tab === "journal") renderJournal();
  } catch (e) { /* offline; the WS frame will fill it in */ }
}

async function fetchTasks() {
  const room = state.room;
  if (!room) return;
  try {
    const [r, g, a, w] = await Promise.all([
      fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/tasks`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/goals`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/attentions`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/waits`, { headers: adminHeaders({}) }),
    ]);
    const list = await r.json();
    const goals = g.ok ? await g.json() : [];
    const attentions = a.ok ? await a.json() : [];
    const waits = w.ok ? await w.json() : [];
    // A slow response for the room we just left must never repaint the room
    // currently at the table.
    if (state.room !== room) return;
    state.tasks = {};
    for (const t of list) state.tasks[t.id] = t;
    state.goals = {};
    for (const goal of goals) state.goals[goal.id] = goal;
    state.attentions = {};
    for (const attention of attentions) state.attentions[attention.id] = attention;
    state.waits = {};
    for (const wait of waits) state.waits[wait.waiter] = wait;
  } catch (e) {}
  if (state.room !== room) return;
  markLocaContextReady("goals", room);
  // The compact Goal strip is room-wide, so it must update even while Chat is
  // open. The rest of the Focus panel is harmless while hidden.
  renderTasks();
  renderReminderHistory();
  rebuildReminderChatProjection();
  if (state.tab === "journal") renderJournal();
}

// Reminder delivery is durable Attention state. The complete lifecycle stays
// here; Chat projects only the newest actionable reminder so an audit trail
// never turns into a wall of repeated messages.
function renderReminderHistory() {
  const reminders = Object.values(state.attentions)
    .filter(attention => ["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"].includes(attention.reason))
    .sort((a, b) => Number(a.created_at || 0) - Number(b.created_at || 0));
  const grouped = new Map();
  for (const reminder of reminders) {
    const key = [reminder.reason, reminder.subject, reminder.owner || ""].join("\u0000");
    const previous = grouped.get(key);
    grouped.set(key, {
      latest: reminder,
      count: (previous?.count || 0) + 1,
    });
  }
  const history = [...grouped.values()]
    .sort((a, b) => Number(a.latest.created_at || 0) - Number(b.latest.created_at || 0))
    .slice(-10)
    .reverse();
  $("reminderHistory").classList.toggle("hidden", !history.length);
  $("reminderHistoryCount").textContent = history.length ? `· ${history.length}` : "0";
  const list = $("reminderHistoryList");
  list.innerHTML = "";
  for (const { latest: reminder, count } of history) {
    const row = document.createElement("div");
    row.className = `reminderhistoryrow ${reminder.status}`;
    const owner = reminder.owner ? `@${reminder.owner}` : "waiting for a healthy recipient";
    const delivery = reminder.delivered_at ? "delivered" : "pending";
    const fallbackStalled = reminder.owner === "loca-care";
    const lifecycle = reminder.status === "resolved" ? "FINISHED"
      : fallbackStalled ? "STALLED"
      : Number(reminder.attempt || 0) > 1 ? "OVERDUE" : "RUNNING";
    const occurrences = count > 1 ? ` · ${count} occurrences` : "";
    row.innerHTML = `<span class="rhsubject"></span><span class="rhmeta"></span>`
      + `<span class="rhstate"></span>`
      + (isLocaOperator() && reminder.status !== "resolved"
        ? `<button data-reminder-resolve="${esc(reminder.id)}">Resolve</button>` : "");
    row.querySelector(".rhsubject").textContent = reminder.subject;
    const attempt = ` · attempt ${Math.max(1, Number(reminder.attempt || 1))}`;
    row.querySelector(".rhmeta").textContent = `${owner} · ${delivery}${reminderTiming(reminder)}${attempt}${occurrences} · last ${fmtFull(reminder.created_at)}`;
    row.querySelector(".rhstate").textContent = lifecycle;
    list.appendChild(row);
  }
}
const TLABEL = { open: "To do", taken: "In progress", done: "Done", cancelled: "Removed" };
/// The building from above: who belongs, where they sit, where they are barred.
///
/// Bans are per-loca, so without this the master had to walk into each room to
/// remember who was barred where — and a ban nobody can find is a ban nobody
/// can lift.
function renderJournal() {
  const box = $("journalList");
  if (!state.journal.length) {
    box.innerHTML = `<div class="sysline">nothing recorded yet — the journal is what was already done, written by whoever did it</div>`;
    return;
  }
  // Newest first: the last thing that happened is the thing you came to read.
  const rows = [...state.journal].sort((a, b) => b.id - a.id);
  let html = "", day = "";
  for (const e of rows) {
    const d = new Date(e.at);
    const label = d.toDateString();
    if (label !== day) { day = label; html += `<div class="jday">${esc(dayLabel(e.at))}</div>`; }
    const t = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const glyph = e.by_type === "agent" ? "*" : ".";
    html += `<div class="jrow"><span class="jwho">${glyph}${esc(e.by)}</span>` +
            `<span class="jtxt">${esc(e.text)}</span><span class="jat">${t}</span></div>`;
  }
  box.innerHTML = html;
}

function renderTasks() {
  const goals = Object.values(state.goals).sort((a, b) => b.id - a.id);
  const activeGoal = goals.find(g => g.status === "active");
  renderLocaSidebar();
  $("goalBar").classList.toggle("hidden", !activeGoal);
  if (!activeGoal) {
    $("goalCard").innerHTML = "";
    $("goalPanelCard").innerHTML = `<span>No room goal yet.</span>${isLocaOperator() ? `<button data-goal-prefill>Set with @goal</button>` : ""}`;
  } else {
    const acts = isLocaOperator()
      ? `<button data-gact="edit" data-gid="${activeGoal.id}">edit</button><button data-gact="achieve" data-gid="${activeGoal.id}">done</button><button data-gact="cancel" data-gid="${activeGoal.id}">remove</button>`
      : "";
    const proof = activeGoal.checkpoint
      ? `<span class="goalproof" title="Success proof: ${esc(activeGoal.checkpoint)}">✓ ${esc(activeGoal.checkpoint)}</span>` : "";
    $("goalCard").innerHTML = `<span class="goallabel">Room goal</span><span class="goaloutcome" title="${esc(activeGoal.outcome)}">${esc(activeGoal.outcome)}</span>${proof}<span class="tacts">${acts}</span>`;
    $("goalPanelCard").innerHTML = `<span><b>${esc(activeGoal.outcome)}</b>${activeGoal.checkpoint ? `<small style="display:block;color:var(--dim);margin-top:3px">Proof: ${esc(activeGoal.checkpoint)}</small>` : ""}</span>${isLocaOperator() ? `<button data-goal-prefill>Edit with @goal</button>` : ""}`;
  }
  $("taskCreate").classList.toggle("hidden", !isLocaOperator());
  // Everyone can see which reminders are active and who receives them; only
  // the master can change the rules.
  $("reminderSettings").classList.remove("hidden");
  renderReminderSettings();

  const waits = Object.values(state.waits).sort((a, b) => a.waiter.localeCompare(b.waiter));
  $("waitSection").classList.toggle("hidden", waits.length === 0);
  $("waitList").innerHTML = waits.length
    ? waits.map(w => `<div class="sysline">⏳ <b>${esc(w.waiter)}</b> is waiting for <b>${esc(w.waiting_for)}</b>${w.reason ? ` — ${esc(w.reason)}` : ""}${isLocaOperator() ? ` <button data-clearwait="${esc(w.waiter)}">Clear</button>` : ""}</div>`).join("")
    : "";
  const box = $("taskList");
  const ids = Object.keys(state.tasks).map(Number).sort((a, b) => b - a);
  if (!ids.length) { box.innerHTML = `<div class="sysline">No explicit tasks. Most work can stay in Chat.</div>`; return; }
  box.innerHTML = "";
  for (const id of ids) {
    const t = state.tasks[id];
    const mine = t.assigned_to === state.name;
    let acts = "";
    if (t.status === "open" && (mine || !t.assigned_to)) acts += `<button data-tact="take" data-tid="${id}">Start</button>`;
    if (t.status === "taken" && mine) acts += `<button data-tact="done" data-tid="${id}">Done</button>`;
    if (isLocaOperator()) {
      if (t.status === "open" || t.status === "taken") acts += `<button data-tact="cancel" data-tid="${id}">Remove</button>`;
      if (t.status === "done" || t.status === "cancelled") acts += `<button data-tact="reopen" data-tid="${id}">Reopen</button>`;
    }
    const el = document.createElement("div");
    el.className = "task";
    el.innerHTML = `<span class="tid">#${id}</span>
      <span class="ttitle">${esc(t.title)}
        <span class="tmeta">Added by ${esc(t.created_by)}${t.assigned_to ? " · " + esc(t.assigned_to) : ""}${t.from_message ? ` · <a href="#" data-goto="${t.from_message}" style="color:var(--accent2)">source message</a>` : ""}</span>
      </span>
      <span class="tchip ${t.status}">${TLABEL[t.status] || t.status}</span>
      <span class="tacts">${acts}</span>`;
    box.appendChild(el);
  }
}

async function taskAct(act, id) {
  const t = state.tasks[id];
  const body = { by: state.name };
  if (act === "take") { body.status = "taken"; if (!t.assigned_to) body.assigned_to = state.name; }
  if (act === "done") body.status = "done";
  if (act === "cancel") body.status = "cancelled";
  if (act === "reopen") body.status = "open";
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/tasks/${id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
  });
  if (r.status === 403) addSys("house rules: take/done belong to the assignee; remove/reopen to the operator");
}
async function createTask(title, fromMessage, assignee) {
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/tasks`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ title, by: state.name, from_message: fromMessage || null, assigned_to: assignee || null }),
  });
  if (r.status === 403) { addSys("Only a loca operator can add a next step."); return false; }
  return r.ok;
}
function currentGoal() {
  return Object.values(state.goals).find(goal => goal.status === "active") || null;
}
async function applyGoalCommand(outcome) {
  const active = currentGoal();
  if (outcome === null && !active) {
    addSys("This loca has no active goal.");
    return true;
  }
  if (outcome !== null && !active && !state.settings?.lead) {
    addSys("Goal cannot be activated · Select a Lead first.");
    return false;
  }
  const updating = !!active;
  const path = updating
    ? `/rooms/${encodeURIComponent(state.room)}/goals/${active.id}`
    : `/rooms/${encodeURIComponent(state.room)}/goals`;
  const payload = outcome === null
    ? { status: "cancelled", by: state.name }
    : updating
      ? { outcome, by: state.name }
      : { outcome, checkpoint: null, completion: "manual", task_ids: [], by: state.name };
  const r = await fetch(`${serverBase()}${path}`, {
    method: updating ? "PATCH" : "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(payload),
  });
  if (!r.ok) {
    addSys("Could not update the room goal: " + await r.text());
    return false;
  }
  const goal = await r.json();
  state.goals[goal.id] = goal;
  renderTasks();
  addSys(outcome === null ? "Room goal removed." : `Room goal: ${outcome}`);
  return true;
}
async function goalAct(action, id) {
  const status = action === "achieve" ? "achieved" : "cancelled";
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/goals/${id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ status, by: state.name }),
  });
  if (!r.ok) { addSys("Could not update the shared outcome: " + await r.text()); return; }
  const goal = await r.json(); state.goals[goal.id] = goal; renderTasks();
}
async function attentionAct(action, id) {
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/attentions/${encodeURIComponent(id)}/${action}`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ by: state.name }),
  });
  if (!r.ok) { addSys("Could not update the reminder: " + await r.text()); return; }
  const attention = await r.json(); state.attentions[attention.id] = attention;
  renderReminderSettings();
  renderReminderHistory();
}
async function clearWait(name) {
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/waits/${encodeURIComponent(name)}`, {
    method: "DELETE", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ by: state.name }),
  });
  if (!r.ok) { addSys("could not clear wait: " + await r.text()); return; }
  delete state.waits[name]; renderTasks();
}

/* ---- tabs ---- */
function switchTab(tab) {
  state.tab = tab;
  const global = tab === "people";
  document.querySelector(".main").classList.toggle("global", global);
  if (global) {
    $("curRoom").textContent = "Building";
    const people = state.people || [];
    const lobby = people.filter(p => !(p.locas || []).length).length;
    $("subStatus").textContent = `${people.length} members · ${lobby} in lobby`;
  } else {
    $("curRoom").textContent = state.room || "—";
  }
  $("tabChat").classList.toggle("active", tab === "chat");
  $("tabNotes").classList.toggle("active", tab === "notes");
  $("tabTasks").classList.toggle("active", tab === "tasks");
  $("tabJournal").classList.toggle("active", tab === "journal");
  $("tabPeople").classList.toggle("active", tab === "people");
  document.querySelectorAll(".tabs [role=tab]").forEach((button) => {
    button.setAttribute("aria-selected", String(button.dataset.tab === tab));
  });
  const chat = tab === "chat";
  $("feed").classList.toggle("hidden", !chat);
  $("composer").classList.toggle("hidden", !chat);
  $("hint").classList.toggle("hidden", !chat);
  renderPinned(); // the pinned bar shows only on the chat tab
  $("notesPanel").classList.toggle("hidden", tab !== "notes");
  $("tasksPanel").classList.toggle("hidden", tab !== "tasks");
  $("journalPanel").classList.toggle("hidden", tab !== "journal");
  $("peoplePanel").classList.toggle("hidden", tab !== "people");
  if (tab === "tasks") { $("tasksDot").classList.remove("on"); renderTasks(); }
  if (tab === "journal") { $("journalDot").classList.remove("on"); fetchJournal(); }
  if (tab === "people") fetchPeople();
  $("typing").classList.toggle("hidden", !chat);
  if (tab === "notes") { $("notesDot").classList.remove("on"); renderNotes(); }
  else {
    // Coming back to chat: if the feed was cleared (e.g. a reconnect happened
    // while we were on Notes), repaint it from the kept transcript.
    if (!$("feed").children.length && state.msgs.length) repaintFeed();
    scrollFeed(true);
    setTimeout(clearUnreadBoundary, 4000);   // seen it — let it go
  }
  if (tab === "chat") markRoomRead(state.room, state.lastId);
}

/* ---- notes ---- */
