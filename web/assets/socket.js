"use strict";
// WebSocket lifecycle, frame dispatch, typing, moderation, and lead control.
let connOn = false;
function setStatus(txt, on) {
  connOn = !!on;
  $("connDot").classList.toggle("on", connOn);
  $("connDot").title = txt;
  $("connDot").setAttribute("aria-label", txt);
  renderTopStatus();
}

// WhatsApp-style live subtitle: connection / online counts / mode / typing.
function renderTopStatus() {
  const el = $("subStatus");
  if (!el) return;
  if (!state.room) { el.textContent = "not connected"; return; }
  const typers = getTypers();
  if (typers.length) {
    el.innerHTML = `<span class="typ">${typers.map(esc).join(", ")} typing…</span>`;
    return;
  }
  if (!connOn) { el.textContent = "connecting…"; return; }
  const agents = state.members.filter(m => m.type === "agent").length;
  const users = state.members.length - agents;
  const m = state.mode || { mode: "free" };
  let modeTxt = m.mode === "free" ? "" :
    m.mode === "paused" ? " · ⏸ paused" :
    m.mode === "restricted" ? " · 🔒 restricted" :
    m.mode === "roundrobin" ? ` · 🔁 turn: ${esc((m.order||[])[m.turn] || "?")}` : "";
  el.innerHTML = `<span class="live">${state.members.length} at the table</span> · <span style="color:var(--human)">.${users}</span> <span style="color:var(--agent)">*${agents}</span>${modeTxt}`;
}

function reminderTiming(attention) {
  const settings = state.settings || {};
  const thresholdByReason = {
    goal_reminder: Number(settings.care_goal_secs || 0),
    task_reminder: Number(settings.care_task_secs || 0),
    wait_overdue: Number(settings.care_wait_secs || 0),
    wait_cycle: Number(settings.care_wait_secs || 0),
    room_silence: Number(settings.care_silence_secs || 0),
  };
  const threshold = thresholdByReason[attention.reason] || 0;
  const retry = Number(settings.care_cooldown_secs || 0);
  const attempt = Math.max(1, Number(attention.attempt || 1));
  const elapsed = threshold + Math.max(0, attempt - 1) * retry;
  const minutes = Math.max(1, Math.ceil(elapsed / 60));
  const thresholdMinutes = Math.max(1, Math.ceil(threshold / 60));
  const next = retry > 0 ? ` · next check ${Math.ceil(retry / 60)} min` : "";
  return ` · waiting ${minutes} min · threshold ${thresholdMinutes} min${next}`;
}

function boundedChatText(value, max = 120) {
  const text = String(value || "");
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

function reminderChatText(attention) {
  const owner = attention.owner ? `@${attention.owner}` : "a healthy recipient";
  const timing = reminderTiming(attention).match(/waiting [^·]+/)?.[0]?.trim();
  const subject = boundedChatText(attention.subject || "Reminder");
  return `${owner}, ${subject}${timing ? ` · ${timing}` : ""}`;
}

function addReminderChatBubble(attention) {
  if (!attention.delivered_at || !attention.owner) return false;
  const attempt = Math.max(1, Number(attention.attempt || 1));
  const configuredMax = Number(state.settings?.care_max_attempts);
  if (Number.isFinite(configuredMax) && configuredMax > 0 && attempt > configuredMax) return false;
  // One visible message per bounded attempt. A replayed Attention frame keeps
  // the same durable identity + attempt and must not duplicate the bubble.
  const deliveryKey = `${state.room || attention.room || ""}\u0000${attention.id}\u0000${attempt}`;
  if (state.reminderReceipts.has(deliveryKey)) return false;
  state.reminderReceipts.add(deliveryKey);
  addMsg({
    sender: "loca",
    sender_type: "agent",
    target: attention.owner,
    text: reminderChatText(attention),
    kind: "reminder",
    ts: attention.delivered_at || attention.created_at,
  });
  return true;
}

function resetReminderChatProjection() {
  document.querySelectorAll("#feed .row.locareminder").forEach(row => row.remove());
  state.reminderReceipts.clear();
}

function rebuildReminderChatProjection() {
  resetReminderChatProjection();
  Object.values(state.attentions)
    .filter(attention => ["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"]
      .includes(attention.reason))
    .filter(attention => !attention.room || attention.room === state.room)
    .sort((a, b) => Number(a.created_at || 0) - Number(b.created_at || 0))
    .forEach(addReminderChatBubble);
}

function reminderLifecycle(attention) {
  if (attention.status === "resolved") return "FINISHED";
  if (attention.owner === "loca-care") return "STALLED";
  if (Number(attention.attempt || 0) > 1 || attention.escalated) return "OVERDUE";
  return "RUNNING";
}

function goalChatReceipt(previous, goal) {
  if (!previous) return "";
  const outcome = boundedChatText(goal.outcome);
  if (previous.status !== goal.status) {
    if (goal.status === "achieved") return `loca · Goal finished: ${outcome}`;
    if (goal.status === "cancelled") return `loca · Goal closed: ${outcome}`;
    if (goal.status === "active") return `loca · Goal continues: ${outcome}`;
  }
  if (goal.status === "active" && Number(goal.progress_at || 0) > Number(previous.progress_at || 0)) {
    return `loca · Goal continues: ${outcome}`;
  }
  return "";
}

function joinRoom(room) {
  if (state.tab === "people") switchTab("chat");
  state.room = room;
  resetLocaContext(room);
  state.locaOperator = null;
  state.settings = {};
  state.goals = {};
  state.profile = null;
  state.members = [];
  state.seatedAway = [];
  state.mode = { mode: "free" };
  setSidebarView("loca");
  renderLocaSidebar();
  renderProfile();
  renderSettings();
  renderTasks();
  renderMembers();
  renderMode();
  markRoomRead(room, state.roomLatest[room]);
  $("curRoom").textContent = room;
  $("roomAvatar").textContent = (room[0] || "#").toUpperCase();
  $("feed").innerHTML = "";
  state.attentions = {};
  resetReminderChatProjection();
  state.notes = {};
  $("notesDot").classList.remove("on");
  refreshRooms();
  fetchNotes();
  fetchMode();
  fetchSettings();
  fetchMod();
  fetchTasks();
  fetchJournal();
  fetchSeated();
  fetchLobby();
  fetchProfile();
  fetchLocaSidebar();
  openWs();
}

function openWs() {
  if (state.ws) { state.ws.onclose = null; state.ws.close(); }
  if (!state.room) return;
  const protocols = ["loca.v1"];
  if (state.session) protocols.push(`loca.session.${state.session}`);
  else if (state.roomToken) protocols.push(`loca.room.${state.roomToken}`);
  // Bearers travel in the WebSocket protocol header, never in URLs that may
  // be retained by proxies, APM, browser history, or access logs. The server
  // negotiates only the non-secret `loca.v1` protocol in its response.
  const url = `${wsBase()}/ws?room=${encodeURIComponent(state.room)}&name=${encodeURIComponent(state.name)}&type=user`;
  const ws = new WebSocket(url, protocols);
  state.ws = ws;
  setStatus("connecting…", false);
  // On (re)connect the server resends full history; clear the feed and the
  // dedup set so we rebuild cleanly instead of stacking duplicates. (B1)
  ws.onopen = () => {
    setStatus("connected", true);
    // If the reader is up in history, keep their place through the resync.
    const f = $("feed");
    keepScroll = nearBottom() ? null : f.scrollTop;
    // The server replays full history on (re)connect. Reset the rendered feed
    // AND the dedup set together so history repaints cleanly — messages live in
    // state.msgs, so nothing is lost even if we're on another tab right now.
    state.msgs = [];
    state.seen = new Set();
    state.lastId = 0;
    lastDayKey = null;
    $("feed").innerHTML = "";
    resetReminderChatProjection();
  };
  ws.onclose = () => { setStatus("connection lost — pulling your chair back…", false); if (state.room) setTimeout(openWs, 1500); };
  ws.onmessage = (ev) => {
    if (state.ws !== ws) return;
    onFrame(JSON.parse(ev.data));
  };
}

function onFrame(f) {
  if (f.t === "history") {
    for (const m of f.messages) addMsg(m);
    rebuildReminderChatProjection();
    renderReminderHistory();
    markRoomRead(state.room, state.lastId);
    if (!f.messages.length) addSys("an empty table — nobody has spoken here yet. words wait.");
    if (keepScroll !== null) { $("feed").scrollTop = keepScroll; keepScroll = null; }
    else scrollFeed(true);
  }
  else if (f.t === "msg") {
    const mine = f.message.sender === state.name;
    const mid = Number(f.message.id || 0);
    state.roomLatest[state.room] = Math.max(Number(state.roomLatest[state.room] || 0), mid);
    if (mine || (state.tab === "chat" && !document.hidden)) {
      markRoomRead(state.room, mid);
    } else if (mid > Number(state.readCursors[state.room] || 0)) {
      state.unread[state.room] = Number(state.unread[state.room] || 0) + 1;
      state.unreadChecked[state.room] = mid;
      renderRooms();
    }
    // WhatsApp rule: decide BEFORE appending. If the reader was at (or near)
    // the bottom, follow the new message — even a tall one. Only a reader
    // who had scrolled up into history stays put.
    const wasAtBottom = nearBottom();
    if (!mine && (state.tab !== "chat" || document.hidden)) markUnreadBoundary();
    addMsg(f.message);
    const followed = mine || wasAtBottom;
    scrollFeed(followed);
    if (!followed && !mine) bumpJump();       // WhatsApp's "↓ new messages"
  }
  else if (f.t === "members") { state.members = f.members; renderMembers(); fetchSeated(); fetchLobby(); }
  else if (f.t === "control") {
    if (f.cmd && f.cmd.startsWith("note-deleted:")) { const k = f.cmd.slice("note-deleted:".length); delete state.notes[k]; if (state.tab === "notes") renderNotes(); }
    else if (f.cmd && f.cmd.startsWith("live-expired:")) { addSys(`⏱ live mode auto-disabled after ${f.cmd.split(":")[1]}s of silence`); }
    else if (f.cmd === "room-closed") { addSys(`this loca was closed — moving you to ${state.homeRoom}`); setTimeout(() => doConnect(state.homeRoom), 800); }
    else { addSys("control: /" + f.cmd); scrollFeed(); }
  }
  else if (f.t === "note") { onNoteFrame(f.note); }
  else if (f.t === "notewarn") {
    addSys(`⚠ ${f.by} edited note "${f.key}" but can_write = [${f.can_write.join(", ")}]`);
    if (state.tab === "chat") scrollFeed();
  }
  else if (f.t === "mode") { state.mode = f.mode; renderMode(); }
  else if (f.t === "settings") {
    // A new lead changes how every message by that name reads, so the
    // transcript has to be redrawn — the title is worn, not announced once.
    const leadChanged = state.settings?.lead !== f.settings?.lead;
    state.settings = f.settings;
    markLocaContextReady("settings");
    renderSettings();
    if (leadChanged) { repaintFeed(); renderMembers(); fetchProfile(); }
  }
  else if (f.t === "typing") { onTyping(f.name, f.on); }
  else if (f.t === "task") {
    state.tasks[f.task.id] = f.task;
    if (state.tab === "tasks") renderTasks(); else $("tasksDot").classList.add("on");
  }
  else if (f.t === "goal") {
    const previous = state.goals[f.goal.id];
    state.goals[f.goal.id] = f.goal;
    markLocaContextReady("goals");
    const receipt = goalChatReceipt(previous, f.goal);
    if (receipt) addSys(receipt);
    renderTasks();
    if (state.tab !== "tasks") $("tasksDot").classList.add("on");
  }
  else if (f.t === "attention") {
    // The server fences deliveries by room; keep the browser boundary closed
    // too so a stale/replayed frame cannot paint an orange reminder in the
    // loca currently on screen.
    if (f.attention.room && f.attention.room !== state.room) return;
    state.attentions[f.attention.id] = f.attention;
    const isReminder = ["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"]
      .includes(f.attention.reason);
    if (isReminder) addReminderChatBubble(f.attention);
    renderReminderSettings();
    renderReminderHistory();
  }
  else if (f.t === "wait") {
    if (f.wait) state.waits[f.waiter] = f.wait; else delete state.waits[f.waiter];
    if (state.tab === "tasks") renderTasks();
  }
  else if (f.t === "care") {
    // Chat shows one delivered Reminder receipt. The complete durable
    // lifecycle remains in Focus/History; Care itself stays silent here.
    renderReminderSettings();
  }
  else if (f.t === "journal") {
    state.journal.push(f.entry);
    if (state.tab === "journal") renderJournal();
  if (state.tab === "people") fetchPeople(); else $("journalDot").classList.add("on");
  }
  else if (f.t === "mod") { state.mod = f.state; renderMembers(); }
  else if (f.t === "kicked") {
    if (f.name === state.name) {
      addSys(f.banned ? "you were banned from this loca" : "you were kicked from the table");
      if (state.ws) { state.ws.onclose = null; state.ws.close(); }
      setStatus(f.banned ? "banned" : "kicked", false);
    }
  }
}

/* ---- typing indicator ---- */
const typers = new Map();  // name -> expiry timer
function onTyping(name, on) {
  if (name === state.name) return;
  if (typers.has(name)) clearTimeout(typers.get(name));
  if (on) {
    typers.set(name, setTimeout(() => { typers.delete(name); renderTyping(); }, 4000));
  } else {
    typers.delete(name);
  }
  renderTyping();
}
async function fetchMod() {
  if (!state.room) return;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/moderate`, { headers: adminHeaders({}) });
    state.mod = await r.json();
  } catch (e) {}
  renderMembers();
}
async function moderate(action, name) {
  if ((action === "kick" || action === "ban") && !confirm(`${action} ${name}?`)) return;
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/moderate`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ action, name }),
  });
  if (r.status === 401) alert("admin token required");
  if (r.ok && ["release", "kick", "ban"].includes(action)) {
    fetchLobby();
    fetchSeated();
  }
}

async function setLead(name) {
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/lead`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ lead: name }),
  });
  if (!r.ok) {
    addSys(`could not ${name ? "name" : "end"} lead: ${await r.text()}`);
    return false;
  }
  const settings = await r.json().catch(() => null);
  if (settings) state.settings = settings;
  renderSettings();
  renderMembers();
  return true;
}

function renderLeadControl() {
  const select = $("leadSelect");
  if (!select) return;
  $("leadKey").classList.toggle("hidden", !isAdmin() || !state.room);
  select.classList.toggle("hidden", !isAdmin() || !state.room);
  if (!isAdmin() || !state.room) return;

  const current = state.settings?.lead || "";
  const names = [...new Set([
    ...state.members.map(member => member.name),
    ...(state.seatedAway || []).map(member => member.name),
    ...(current ? [current] : []),
  ])].sort((a, b) => a.localeCompare(b));
  select.innerHTML = `<option value="">◇ no lead</option>` + names.map(name =>
    `<option value="${esc(name)}">◆ ${esc(name)}</option>`
  ).join("");
  select.value = current;
  select.title = current
    ? `${current} is lead — choose another name to transfer, or no lead to end`
    : "choose this loca's lead";
}

// `@lead` is an explicit room-control command, not ordinary chat. Keeping the
// grammar here means prose can never mutate room state, while the command the
// room documents (`@lead <name>` / `@lead none`) uses the same authenticated
// endpoint as the sidebar diamond.
function parseLeadCommand(text) {
  if (!/^@lead(?:\s|$)/i.test(text)) return null;
  const parts = text.trim().split(/\s+/);
  if (parts.length !== 2 || !parts[1]) return { error: "usage: @lead <name> · @lead none" };
  if (parts[1].toLowerCase() === "none") return { lead: null };

  const seated = [
    ...state.members,
    ...(state.seatedAway || []),
  ].map(member => member.name);
  const matches = seated.filter(name => name.toLowerCase() === parts[1].toLowerCase());
  if (matches.length !== 1) {
    return { error: `lead must be one seated person; "${parts[1]}" was not found` };
  }
  return { lead: matches[0] };
}

// Goal is a room-level outcome, not a chat message or a task form. The command
// is intentionally tiny and explicit so ordinary conversation can never
// manufacture workflow state.
function parseGoalCommand(text) {
  if (!/^@goal(?:\s|$)/i.test(text)) return null;
  let outcome = text.trim().replace(/^@goal(?:\s+|$)/i, "").trim();
  if (!outcome) return { error: "usage: @goal <outcome> · @goal none" };
  if (outcome.toLowerCase() === "none") return { outcome: null };
  if (outcome.length >= 2) {
    const first = outcome[0], last = outcome[outcome.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      outcome = outcome.slice(1, -1).trim();
    }
  }
  if (!outcome) return { error: "usage: @goal <outcome> · @goal none" };
  return { outcome };
}

function getTypers() { return [...typers.keys()]; }
function renderTyping() {
  const names = getTypers();
  const el = $("typing");
  el.textContent = !names.length ? "" :
    names.length === 1 ? `${names[0]} is typing…` : `${names.slice(0, 2).join(", ")}${names.length > 2 ? " +" : ""} typing…`;
  renderTopStatus();   // typing also surfaces in the WhatsApp-style header
}
let typingSent = false, typingStop = null;
function signalTyping() {
  if (!state.ws || state.ws.readyState !== 1) return;
  if (!typingSent) { state.ws.send(JSON.stringify({ t: "typing", on: true })); typingSent = true; }
  if (typingStop) clearTimeout(typingStop);
  typingStop = setTimeout(() => {
    if (state.ws && state.ws.readyState === 1) state.ws.send(JSON.stringify({ t: "typing", on: false }));
    typingSent = false;
  }, 2500);
}

let unreadMark = false;   // a message arrived while we weren't looking
function markUnreadBoundary() {
  if (unreadMark) return;
  unreadMark = true;
  const d = document.createElement("div");
  d.className = "unread-line"; d.id = "unreadLine";
  d.textContent = "unread";
  $("feed").appendChild(d);
}
function clearUnreadBoundary() {
  unreadMark = false;
  document.getElementById("unreadLine")?.remove();
}
