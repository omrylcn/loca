"use strict";
// Journal, goals, tasks, waits, and bounded reminders.
async function fetchJournal() {
  if (!state.room || !extensionOn("journal")) return;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/journal`, { headers: adminHeaders({}) });
    if (!r.ok) return;
    state.journal = await r.json();
    if (state.tab === "journal") renderJournal();
  } catch (e) { /* offline; the WS frame will fill it in */ }
}

// Live task and goal frames that arrive while a read of the lists is on its
// way. The read's answer is older than they are, so they are laid over it
// instead of being wiped by it.
const liveDuringRead = new Set();
function noteLiveFrame(kind, item) {
  for (const live of liveDuringRead) live[kind][item.id] = item;
}

async function fetchTasks() {
  const room = state.room;
  if (!room) return;
  const live = { tasks: {}, goals: {} };
  liveDuringRead.add(live);
  try {
    // Goals are Focus's and tasks the Loca Board's; attentions and waits are
    // care and stay on in every loca.
    const focus = extensionOn("focus");
    const board = extensionOn("board");
    const none = { ok: true, json: async () => [] };
    const [r, g, a, w] = await Promise.all([
      board ? fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/tasks`, { headers: adminHeaders({}) }) : none,
      focus ? fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/goals`, { headers: adminHeaders({}) }) : none,
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
    Object.assign(state.tasks, live.tasks);
    state.goals = {};
    for (const goal of goals) state.goals[goal.id] = goal;
    Object.assign(state.goals, live.goals);
    state.attentions = {};
    for (const attention of attentions) state.attentions[attention.id] = attention;
    state.waits = {};
    for (const wait of waits) state.waits[wait.waiter] = wait;
  } catch (e) {
  } finally {
    liveDuringRead.delete(live);
  }
  if (state.room !== room) return;
  markLocaContextReady("goals", room);
  refreshTaskBadges();
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
      + ((isLocaOperator() || reminder.owner?.toLowerCase() === state.name.toLowerCase())
          && reminder.status !== "resolved"
        ? `<button data-reminder-resolve="${esc(reminder.id)}">${isLocaOperator() ? "Resolve" : "No action needed"}</button>` : "");
    row.querySelector(".rhsubject").textContent = reminder.subject;
    const attempt = ` · attempt ${Math.max(1, Number(reminder.attempt || 1))}`;
    row.querySelector(".rhmeta").textContent = `${owner} · ${delivery}${reminderTiming(reminder)}${attempt}${occurrences} · last ${fmtFull(reminder.created_at)}`;
    row.querySelector(".rhstate").textContent = lifecycle;
    list.appendChild(row);
  }
}
const TLABEL = { open: "To do", taken: "In progress", review: "Review", done: "Done", cancelled: "Removed" };

// The number of the unfinished task a person holds, worn beside their name in
// the roster and on their lines. A task belongs to the principal the server
// recorded for it (`assigned_principal`), never to whoever wears the name now;
// a task given by name alone (the open sandbox, an older record) is worn only
// by a seat or a line nobody verified. Inside this loca only.
function taskBadgeFor(name, principal) {
  if (!name || isDm(state.room)) return "";
  const rank = t => ({ taken: 0, review: 1 }[t.status] ?? 2);
  const held = Object.values(state.tasks || {})
    .filter(t => t.room === state.room && ["open", "taken", "review"].includes(t.status) && t.assigned_to
      && (t.assigned_principal ? t.assigned_principal === principal : (!principal && t.assigned_to === name)))
    .sort((a, b) => rank(a) - rank(b) || a.id - b.id);
  if (!held.length) return "";
  const more = held.length > 1 ? ` +${held.length - 1}` : "";
  const tip = held.map(t => `#${t.id} ${t.title} (${TLABEL[t.status]})`).join("\n");
  return ` <span class="tasktag" title="${esc(tip)}" data-task-ref="${held[0].id}" role="link" tabindex="0">#${held[0].id}${more}</span>`;
}

// Tasks changed: redraw the numbers people wear.
function refreshTaskBadges() {
  renderMembers();
  refreshSenderTags();
}
/// The building from above: who belongs, where they sit, where they are barred.
///
/// Bans are per-loca, so without this the master had to walk into each room to
/// remember who was barred where — and a ban nobody can find is a ban nobody
/// can lift.
// Whether a name seated in this loca (here or away) is an agent's. Only for
// how a name is drawn: a star for an agent, a dot for a person.
function nameIsAgent(name) {
  if (!name) return false;
  const members = [...(state.members || []), ...(state.seatedAway || [])];
  return members.some(m => m.name === name && (m.type || m.kind) === "agent");
}

// A day in a running sentence: "today", "yesterday" or "3 March".
function dayInLine(ts) {
  const day = dayLabel(ts);
  return day === "Today" || day === "Yesterday" ? day.toLowerCase() : day;
}

// "since 11:07" today, "since yesterday", or "since 3 March" further back.
function sinceLabel(ts) {
  return dayLabel(ts) === "Today" ? fmtTime(ts) : dayInLine(ts);
}

function renderJournal() {
  const box = $("journalList");
  if (!state.journal.length) {
    box.innerHTML = `<div class="sysline">nothing recorded yet — the journal is what was already done, written by whoever did it</div>`;
    return;
  }
  // Newest first: the last thing that happened is the thing you came to read.
  // Each day is a heading and a list; each entry wears its writer's face.
  const rows = [...state.journal].sort((a, b) => b.id - a.id);
  let html = "", day = "";
  for (const e of rows) {
    const label = new Date(e.at).toDateString();
    if (label !== day) {
      if (day) html += `</ol>`;
      day = label;
      html += `<h2 class="jday">${esc(dayLabel(e.at))}</h2><ol class="jlist">`;
    }
    const agent = e.by_type === "agent";
    html += `<li class="jrow ${agent ? "agent" : "person"}">` +
            `<span class="jface">${faceHtml(e.by, agent ? "agent" : "user", null)}</span>` +
            `<div class="jmain"><div class="jline"><b class="jwho">${agent ? "* " : ""}${esc(e.by)}</b>` +
            `<span class="jat" title="${esc(fmtFull(e.at))}">${esc(fmtTime(e.at))}</span></div>` +
            `<p class="jtxt">${esc(e.text)}</p></div></li>`;
  }
  box.innerHTML = html + `</ol>`;
}

function renderTasks() {
  const goals = Object.values(state.goals).sort((a, b) => b.id - a.id);
  const activeGoal = goals.find(g => g.status === "active");
  renderLocaSidebar();
  $("goalBar").classList.toggle("hidden", !activeGoal);
  if (!activeGoal) {
    $("goalCard").innerHTML = "";
    $("goalPanelMeta").textContent = "";
    $("goalPanelCard").innerHTML = `<span class="goalempty">No room goal yet.</span>${isLocaOperator() ? `<span class="goalacts"><button data-goal-prefill>Set with @goal</button></span>` : ""}`;
  } else {
    const acts = isLocaOperator()
      ? `<button data-gact="edit" data-gid="${activeGoal.id}">edit</button><button data-gact="achieve" data-gid="${activeGoal.id}">done</button><button data-gact="cancel" data-gid="${activeGoal.id}">remove</button>`
      : "";
    const proof = activeGoal.checkpoint
      ? `<span class="goalproof" title="Success proof: ${esc(activeGoal.checkpoint)}">✓ ${esc(activeGoal.checkpoint)}</span>` : "";
    $("goalCard").innerHTML = `<span class="goallabel">Goal:</span><span class="goaloutcome" title="${esc(activeGoal.outcome)}">${esc(activeGoal.outcome)}</span>${proof}<span class="tacts">${acts}</span>`;
    $("goalPanelMeta").textContent = [activeGoal.created_by ? `set by ${activeGoal.created_by}` : "",
      activeGoal.created_at ? dayInLine(activeGoal.created_at) : ""].filter(Boolean).join(" · ");
    $("goalPanelCard").innerHTML = `<span class="goalbody"><b class="goaltext">${esc(activeGoal.outcome)}</b>${activeGoal.checkpoint ? `<small class="goalproofline">Proof: ${esc(activeGoal.checkpoint)}</small>` : ""}</span>`
      + (isLocaOperator() ? `<span class="goalacts"><button data-goal-prefill>Change goal</button><button data-goal-reach="${activeGoal.id}">Mark as reached</button></span>` : "");
  }
  // Everyone can see which reminders are active and who receives them; only
  // the master can change the rules.
  $("reminderSettings").classList.remove("hidden");
  renderReminderSettings();
  refreshContinuityTab();

  const waits = Object.values(state.waits).sort((a, b) => a.waiter.localeCompare(b.waiter));
  $("waitSection").classList.toggle("hidden", waits.length === 0);
  $("waitList").innerHTML = waits.length
    ? waits.map(w => {
      const mark = nameIsAgent(w.waiter)
        ? `<span class="waitmark pagestar" aria-hidden="true">*</span>`
        : `<span class="waitmark waitdot" aria-hidden="true"></span>`;
      const since = w.since ? `<span class="waitsince" title="${esc(fmtFull(w.since))}">since ${esc(sinceLabel(w.since))}</span>` : "";
      return `<div class="sysline waitrow">${mark}<span class="waittext"><span class="waitwho">${esc(w.waiter)}</span> is waiting for <b>${esc(w.waiting_for)}</b>${w.reason ? ` — ${esc(w.reason)}` : ""}</span>${since}${isLocaOperator() ? `<button data-clearwait="${esc(w.waiter)}">Clear</button>` : ""}</div>`;
    }).join("")
    : "";
  // The tasks are the Loca Board's tab; Focus keeps one line about them.
  renderBoardTab();
  renderFocusBoardLine();
  if (typeof refreshHome === "function") refreshHome();   // Home lists the open loca's tasks given to this person
}

async function taskAct(act, id) {
  const t = state.tasks[id];
  const body = { by: state.name };
  if (act === "take") { body.status = "taken"; if (!t.assigned_to) body.assigned_to = state.name; }
  if (act === "done") body.status = "done";
  if (act === "cancel") body.status = "cancelled";
  if (act === "reopen") body.status = "open";
  if (act === "go" || act === "nogo") return boardReview(act, id);
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
  if (r.status === 403) { addSys("Only a loca operator or the Lead can add a next step."); return false; }
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
// The views that are not a loca's: the Building (People) and Home (home.js).
// The loca that was open stays connected behind them.
function isBuildingView(tab = state.tab) { return tab === "people" || tab === "home"; }
function switchTab(tab) {
  state.tab = tab;
  const global = isBuildingView(tab);
  const main = document.querySelector(".main");
  main.classList.toggle("global", global);
  main.classList.toggle("homeview", tab === "home");
  // Home and People belong to no loca: This Loca has nothing to show there,
  // so the sidebar keeps Your Locas and its two tabs step aside.
  document.body.classList.toggle("buildingview-on", global);
  if (global && state.sidebarView === "loca") setSidebarView("building");
  if (tab === "home") {
    $("curRoom").textContent = "Home";
  } else if (global) {
    $("curRoom").textContent = "Building";
    renderBuildingSummary();
  } else {
    $("curRoom").textContent = roomLabel(state.room);
  }
  $("tabChat").classList.toggle("active", tab === "chat");
  $("tabNotes").classList.toggle("active", tab === "notes");
  $("tabTasks").classList.toggle("active", tab === "tasks");
  $("tabBoard").classList.toggle("active", tab === "board");
  $("tabJournal").classList.toggle("active", tab === "journal");
  $("tabWiki").classList.toggle("active", tab === "wiki");
  $("tabVault").classList.toggle("active", tab === "vault");
  $("tabPeople").classList.toggle("active", tab === "people");
  document.querySelectorAll(".tabs [role=tab]").forEach((button) => {
    button.setAttribute("aria-selected", String(button.dataset.tab === tab));
    // The tab bar scrolls sideways when it is full: keep the chosen tab in it.
    if (button.dataset.tab === tab && typeof button.scrollIntoView === "function") {
      button.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  });
  const chat = tab === "chat";
  $("feed").classList.toggle("hidden", !chat);
  $("composer").classList.toggle("hidden", !chat);
  $("hint").classList.toggle("hidden", !chat);
  // Who a message reaches is a line of the composer: only where it is.
  $("addressLine")?.classList.toggle("hidden", !chat);
  // The Building view is not a loca's: This Loca steps aside (locainfo.js).
  if (typeof placeLocaInfo === "function") placeLocaInfo();
  renderPinned(); // the pinned bar shows only on the chat tab
  $("notesPanel").classList.toggle("hidden", tab !== "notes");
  $("tasksPanel").classList.toggle("hidden", tab !== "tasks");
  $("boardPanel").classList.toggle("hidden", tab !== "board");
  $("journalPanel").classList.toggle("hidden", tab !== "journal");
  $("memoryPanel").classList.toggle("hidden", tab !== "memory");
  $("wikiPanel").classList.toggle("hidden", tab !== "wiki");
  $("vaultPanel").classList.toggle("hidden", tab !== "vault");
  $("peoplePanel").classList.toggle("hidden", tab !== "people");
  $("homePanel").classList.toggle("hidden", tab !== "home");
  $("homeLink")?.classList.toggle("active", tab === "home");
  $("homeLink")?.setAttribute("aria-current", tab === "home" ? "page" : "false");
  if (tab === "tasks") { $("tasksDot").classList.remove("on"); renderTasks(); }
  if (tab === "board") { $("boardDot").classList.remove("on"); renderTasks(); }
  if (tab === "journal") { $("journalDot").classList.remove("on"); fetchJournal(); }
  if (tab === "people") fetchPeople();
  if (tab === "home") renderHome();
  if (tab === "memory") fetchMemory();
  if (tab === "wiki") fetchWiki();
  if (tab === "vault") fetchVault();
  $("typing").classList.toggle("hidden", !chat);
  if (tab === "notes") { $("notesDot").classList.remove("on"); renderNotes(); }
  // Home is not the chat: the unread boundary waits for the chat to be seen.
  else if (tab !== "home") {
    // Coming back to chat: if the feed was cleared (e.g. a reconnect happened
    // while we were on Notes), repaint it from the kept transcript.
    if (!$("feed").children.length && state.msgs.length) repaintFeed();
    scrollFeed(true);
    setTimeout(clearUnreadBoundary, 4000);   // seen it — let it go
  }
  if (tab === "chat") markRoomRead(state.room, state.lastId);
}

/* ---- notes ---- */

// "Mark as reached" on the Focus goal card: the same act as the goal line's
// "done" (goalAct), offered where the goal is read in full.
$("goalPanelCard").addEventListener("click", (e) => {
  const b = e.target.closest("[data-goal-reach]");
  if (b) goalAct("achieve", Number(b.dataset.goalReach));
});
