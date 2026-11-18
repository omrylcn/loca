"use strict";
let joinedRoomNeedsBottom = false;
// WebSocket lifecycle, frame dispatch, typing, moderation, and lead control.
let connOn = false;
function setStatus(txt, on) {
  connOn = !!on;
  $("connDot").classList.toggle("on", connOn);
  const dot = $("connDot");
  dot.setAttribute("aria-label", txt);
  dot.dataset.tip = connOn ? "Connected" : txt.charAt(0).toUpperCase() + txt.slice(1);
  dot.dataset.tipDetail = connOn
    ? "Live link to the server; messages arrive as they are sent."
    : "Not linked to the server right now; messages will not arrive.";
  refreshTip(dot);
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
  if (!connOn) { el.textContent = steppedDown() ? "open in another tab" : "connecting…"; return; }
  const agents = state.members.filter(m => m.type === "agent").length;
  const users = state.members.length - agents;
  const m = state.mode || { mode: "free" };
  let modeTxt = m.mode === "free" ? "" :
    m.mode === "paused" ? " · ⏸ paused" :
    m.mode === "restricted" ? " · 🔒 restricted" :
    m.mode === "roundrobin" ? ` · 🔁 turn: ${esc((m.order||[])[m.turn] || "?")}` : "";
  if (isDm(state.room)) {
    // Two seats: say whether the other person is here, not a head count.
    const peer = roomLabel(state.room);
    const here = state.members.some(member => member.name !== state.name);
    el.innerHTML = here ? `<span class="live">${esc(peer)} is here</span>` : `${esc(peer)} is not here now`;
    return;
  }
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
  // An "Everyone" reminder fans out to a per-member attention each, all sharing a
  // server-derived `group` (generation) id. Chat shows it as one @all line, not
  // one member's name, so it reads as addressed to the whole loca.
  const owner = attention.group || attention.audience?.kind === "group"
    ? "@all"
    : attention.owner
      ? `@${attention.owner}`
      : attention.audience?.kind === "person"
        ? `@${attention.audience.name}`
        : "@lead";
  const timing = reminderTiming(attention).match(/waiting [^·]+/)?.[0]?.trim();
  const subject = boundedChatText(attention.subject || "Reminder");
  return `${owner}, ${subject}${timing ? ` · ${timing}` : ""}`;
}

function addReminderChatBubble(attention) {
  if (!attention.id) return false;
  const attempt = Math.max(1, Number(attention.attempt || 1));
  // One visible message per bounded attempt. A replayed Attention frame keeps
  // the same durable identity + attempt and must not duplicate the bubble.
  const deliveryKey = `${state.room || attention.room || ""}\u0000${attention.id}\u0000${attempt}`;
  if (state.reminderReceipts.has(deliveryKey)) return false;
  state.reminderReceipts.add(deliveryKey);
  addMsg({
    sender: "loca",
    // An "Everyone" reminder is a single @all line (the text already says @all);
    // it has no one person as target, so addMsg must not prepend an owner
    // mention on top of it. Lead/Person reminders keep their directed target.
    sender_type: "agent",
    target: attention.group ? null : attention.owner || null,
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
  const latest = Object.values(state.attentions)
    .filter(attention => ["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"]
      .includes(attention.reason))
    .filter(attention => !attention.room || attention.room === state.room)
    .sort((a, b) => Number(b.delivered_at || b.created_at || 0)
      - Number(a.delivered_at || a.created_at || 0))[0];
  // Chat is a conversation, not the Reminder audit log. Keep at most the
  // newest delivered reminder visible here without flooding the transcript.
  // Resolution stops retries; it must not erase the visible proof that the
  // reminder was delivered and make a healthy delivery look like a failure.
  // A reconnect rebuilds durable reminders after message history. The newest
  // delivered receipt must remain visible in Chat even when conversation
  // continued afterwards; addMsg places an older reminder at its timestamp
  // instead of falsely appending it as the newest message.
  if (latest) addReminderChatBubble(latest);
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
  cancelLandingRetry();   // opening a loca replaces a landing still being decided (api.js)
  captureNoteDraft();
  if (room !== state.room) closeLocaIdentityOnRoomChange();
  state.editing = null;
  $("noteList").innerHTML = "";
  $("notesError").classList.add("hidden");
  if (state.tab === "people") switchTab("chat");
  state.room = room;
  if (!isDm(room)) { rememberLoca(room); rememberLastLoca(room); }
  joinedRoomNeedsBottom = true;
  resetLocaContext(room);
  state.locaOperator = null;
  state.settings = {};
  state.goals = {};
  state.profile = null;
  state.members = [];
  state.seatedAway = [];
  state.mode = { mode: "free" };
  // Loca is chat-first. Keep the room switcher visible after joining so
  // moving between locas never requires a detour through a detail tab.
  setSidebarView("building");
  // The loca one goes into is never behind a closed floor.
  revealRoomFloor(room);
  renderLocaSidebar();
  renderProfile();
  renderSettings();
  renderTasks();
  renderMembers();
  renderMode();
  markRoomRead(room, state.roomLatest[room]);
  $("curRoom").textContent = roomLabel(room);
  $("roomAvatar").textContent = (roomLabel(room)[0] || "#").toUpperCase();
  revokeInlineAttachmentUrls();
  $("feed").innerHTML = "";
  loadPinned(); // this room's personal pin (localStorage), if any
  state.attentions = {};
  resetReminderChatProjection();
  state.notes = {};
  state.memory = null;
  state.memoryEntries = [];
  state.memoryStatus = "loading";
  state.memoryError = "";
  state.memoryLoading = false;
  resetWikiForRoom();
  $("notesDot").classList.remove("on");
  refreshRooms();
  fetchMemory();
  fetchMode();
  // Settings first: they say which extensions exist here, and nothing is
  // asked of an extension this loca has off.
  fetchExtensionData();
  fetchMod();
  fetchSeated();
  fetchLobby();
  fetchProfile();
  fetchLocaSidebar();
  openWs();
}

// A Building rank renamed its profile. The server names exactly the messages
// that person wrote (`ids`) and the replies that answer one of them
// (`reply_ids`); relabel those and repaint the feed in place. A label is never
// evidence of who wrote a message, so nothing else changes.
function onRelabelled(to, ids, replyIds) {
  if (!to || !Array.isArray(ids) || !Array.isArray(replyIds)) return;
  const written = new Set(ids);
  const answering = new Set(replyIds);
  let changed = false;
  for (const m of state.msgs) {
    if (written.has(m.id) && m.sender !== to) { m.sender = to; changed = true; }
    if (answering.has(m.id) && m.reply_to_sender !== to) { m.reply_to_sender = to; changed = true; }
  }
  if (!changed) return;
  const feed = $("feed");
  const keep = nearBottom() ? null : feed.scrollTop;
  const msgs = state.msgs.slice();
  state.msgs = [];
  state.seen = new Set();
  state.lastId = 0;
  lastDayKey = null;
  feed.innerHTML = "";
  resetReminderChatProjection();
  for (const m of msgs) addMsg(m);
  rebuildReminderChatProjection();
  fetchReactions().catch(() => {});
  if (keep !== null) feed.scrollTop = keep; else scrollFeed(true);
}

// The by-name rename notice older pages relabel with. The messages were
// already relabelled by `relabelled`; if the rename is ours, adopt the name
// and reconnect so this socket routes @mentions to the new name too.
function onRenamed(from, to) {
  if (!from || !to || from === to) return;
  if (state.name === from) {
    adoptOwnName(to);
    openWs();
  }
}

// One name everywhere this browser keeps it: state, the name field and the
// remembered session, so a reload or another tab does not bring the old back.
function adoptOwnName(name) {
  state.name = name;
  if ($("name")) $("name").value = name;
  try {
    const cached = JSON.parse(localStorage.getItem("loca-admin-session") || "null");
    if (cached && cached.token) {
      cached.name = name;
      localStorage.setItem("loca-admin-session", JSON.stringify(cached));
    }
  } catch (e) { /* storage unavailable: the server still knows the name */ }
}

// The server closes a socket whose session has ended, and reconnecting with
// that session fails every time. Ask whether the session is still accepted;
// if it is not, take a new one (renewRefusedSession, joined with the poll's).
async function confirmSession() {
  const session = state.session;
  const gen = state.buildingGen;
  if (!session || !state.roomToken || (sessionRenewal && sessionRenewal.key === renewalKey())) return;
  let r;
  try {
    r = await fetch(serverBase() + "/whoami", { headers: { "x-session-token": session } });
  } catch (e) { return; }
  if (r.status === 401 && !buildingStale(gen) && state.session === session) renewRefusedSession(session);
}

// The one reconnect a lost connection has pending, if any. It belongs to the
// socket that was lost: opening a socket any other way (a renewal, a room
// switch, "Use it here") and stepping down both end it, so a reconnect that
// was scheduled for an earlier socket never opens one that nobody asked for.
let reconnectTimer = null;
function cancelReconnect() {
  if (reconnectTimer !== null) clearTimeout(reconnectTimer);
  reconnectTimer = null;
}
function openWs() {
  cancelReconnect();
  if (state.ws) { state.ws.onclose = null; state.ws.close(); }
  // Connecting is how this tab comes back after it stepped down (onEvicted).
  state.evicted = null;
  renderEvicted();
  if (!state.room) return;
  const gen = state.buildingGen;
  const protocols = ["loca.v1"];
  if (state.session) protocols.push(`loca.session.${state.session}`);
  else if (state.roomToken) protocols.push(`loca.room.${state.roomToken}`);
  // Bearers travel in the WebSocket protocol header, never in URLs that may
  // be retained by proxies, APM, browser history, or access logs. The server
  // negotiates only the non-secret `loca.v1` protocol in its response.
  const watch = isWatchingDoor() ? "&watch=1" : "";
  const url = `${wsBase()}/ws?room=${encodeURIComponent(state.room)}&name=${encodeURIComponent(state.name)}&type=user${watch}`;
  renderDoor();
  const ws = new WebSocket(url, protocols);
  state.ws = ws;
  setStatus("connecting…", false);
  // On (re)connect the server resends full history; clear the feed and the
  // dedup set so we rebuild cleanly instead of stacking duplicates. (B1)
  ws.onopen = () => {
    setStatus("connected", true);
    // If the reader is up in history, keep their place through the resync.
    const f = $("feed");
    keepScroll = joinedRoomNeedsBottom || nearBottom() ? null : f.scrollTop;
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
  ws.onclose = () => {
    setStatus("connection lost — pulling your chair back…", false);
    if (!state.room) return;
    cancelReconnect();
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      // Only for the socket that was lost, and only for the person it was
      // lost for: a socket opened since, or a sign-out or another sign-in
      // at this tab, has taken over.
      if (state.ws !== ws || buildingStale(gen) || steppedDown()) return;
      openWs();
    }, 1500);
    confirmSession();
  };
  ws.onmessage = (ev) => {
    if (state.ws !== ws) return;
    onFrame(JSON.parse(ev.data), { ws, gen });
  };
}

// The server gave this tab's seat to a newer connection of the same identity:
// another tab or device, or the agent runtime holding the same key (last
// writer wins, PRINCIPLES). Reconnecting by itself would take the seat back
// and the two would take it from each other forever, so this tab steps down,
// says why, and comes back only when asked ("Use it here"). The eviction
// belongs to the socket and the person it arrived for: one that lands after
// that socket was replaced, or after a sign-out or another sign-in at this
// tab, is shown to nobody.
function onEvicted(f, from) {
  if (!from || from.ws !== state.ws || buildingStale(from.gen) || !state.room) return;
  cancelReconnect();
  from.ws.onclose = null;
  from.ws.close();
  state.evicted = { gen: from.gen, room: state.room, name: typeof f.name === "string" ? f.name : "" };
  setStatus("open in another tab", false);
  renderEvicted();
}
function steppedDown() {
  return !!state.evicted && !buildingStale(state.evicted.gen) && state.evicted.room === state.room;
}
function renderEvicted() {
  const bar = $("evictedBar");
  if (!bar) return;
  const shown = steppedDown();
  bar.classList.toggle("hidden", !shown);
  if (!shown) return;
  const as = state.evicted.name ? `, as ${state.evicted.name}` : "";
  $("evictedText").textContent = `Another tab or device is now in this loca under this identity${as}. `
    + "This tab stepped out so the two do not keep taking the seat from each other.";
}
// The one way back after stepping down: on purpose, and the other one then
// steps down in turn.
function useItHere() {
  if (!steppedDown()) return;
  openWs();
}

// A host that wraps this UI (the desktop shell) can set window.__LOCA_NOTIFY__
// to raise a native OS notification for the events below. In a plain browser
// the global is undefined and these calls are no-ops. Only WHO + event kind are
// passed here; the message body is deliberately NOT sent (privacy).
const _notifiedKeys = new Set();
function notifyDesktop(kind, sender, loca, id) {
  try {
    // Dedup: a given event (message id / attention id) notifies at most once,
    // even if its frame repeats or a reconnect re-delivers it.
    if (id != null) {
      const key = kind + ":" + id;
      if (_notifiedKeys.has(key)) return;
      _notifiedKeys.add(key);
      if (_notifiedKeys.size > 4000) _notifiedKeys.clear(); // bound memory over long sessions
    }
    if (window.__LOCA_NOTIFY__) {
      window.__LOCA_NOTIFY__({
        kind,
        sender: sender || null,
        loca: loca || null,
        id: id != null ? String(id) : null,
      });
    }
  } catch (e) {}
}
// A message addressed to me: server-set target, or an @name in the body. Pure
// (name passed in) so it is unit-testable without app state.
function isMentionOfMe(m, myName) {
  if (!m || !myName || m.sender === myName) return false;
  if (m.target === myName) return true;
  return !!(m.text && m.text.includes("@" + myName));
}

// What the browser does with every server frame in docs/protocol-frames.json
// (docs/protocol.md). "handled" means onFrame dispatches on it; any other value
// says why it is deliberately ignored. tests/browser/protocol-frames.spec.js
// fails on a frame type missing here: none may fall through unexamined.
const NO_LOBBY_SOCKET = "ignored: the browser opens no Lobby socket";
const WEB_FRAME_HANDLING = {
  history: "handled", msg: "handled", reaction: "handled", members: "handled",
  renamed: "handled", relabelled: "handled", control: "handled", note: "handled", memory: "handled",
  notewarn: "handled", mode: "handled", settings: "handled", typing: "handled",
  task: "handled", goal: "handled", attention: "handled", wait: "handled",
  care: "handled", journal: "handled", mod: "handled", kicked: "handled", evicted: "handled",
  vault: "handled",
  memoryversion: "ignored: a freshness checkpoint for runtimes that inject memory into a model",
  turn: "ignored: sent only on filter=mentions; the browser opens filter=all",
  caretaker: "ignored: never on the wire; the server delivers it as msg",
  lobby_ready: NO_LOBBY_SOCKET,
  called: NO_LOBBY_SOCKET + "; new locas appear through GET /rooms",
  membership_revoked: NO_LOBBY_SOCKET + "; the server ends the browser's session instead",
  membership_suspended: NO_LOBBY_SOCKET + "; the server ends the browser's session instead",
  credential_revoked: NO_LOBBY_SOCKET + "; a revoked key's sessions end with it",
  seats: NO_LOBBY_SOCKET + "; seats appear through GET /rooms",
  notification: NO_LOBBY_SOCKET + "; notifications.js polls GET /notifications",
};

function onFrame(f, from) {
  if (f.t === "history") {
    for (const m of f.messages) addMsg(m);
    rebuildReminderChatProjection();
    renderReminderHistory();
    markRoomRead(state.room, state.lastId);
    if (!f.messages.length) addSys(isDm(state.room)
      ? "nothing said yet — only the two of you will see what is said here."
      : "an empty table — nobody has spoken here yet. words wait.");
    if (keepScroll !== null) { $("feed").scrollTop = keepScroll; keepScroll = null; }
    else scrollFeed(true);
    joinedRoomNeedsBottom = false;
    fetchReactions().catch(() => {});
  }
  else if (f.t === "msg") {
    const mine = f.message.sender === state.name;
    const mid = Number(f.message.id || 0);
    // Native notification only for a message addressed to me (not reactions or
    // ordinary chatter); the desktop shell suppresses it while focused.
    if (isMentionOfMe(f.message, state.name)) notifyDesktop("mention", f.message.sender, state.room, mid);
    soundForLiveMessage(f.message);
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
  else if (f.t === "reaction") {
    applyReactionSummary(f.reaction);
    if (f.reaction.owner === state.name && f.reaction.reactor !== state.name && document.hidden) {
      flashTitle(f.reaction.reactor);
    }
  }
  else if (f.t === "members") { state.members = f.members; renderMembers(); refreshSenderTags(); fetchSeated(); fetchLobby(); }
  else if (f.t === "relabelled") { onRelabelled(f.to, f.ids, f.reply_ids); }
  else if (f.t === "renamed") { onRenamed(f.from, f.to); }
  else if (f.t === "control") {
    if (f.cmd && f.cmd.startsWith("note-deleted:")) { const k = f.cmd.slice("note-deleted:".length); delete state.notes[k]; if (state.tab === "notes") renderNotes(); }
    else if (f.cmd && f.cmd.startsWith("live-expired:")) { addSys(`⏱ live mode auto-disabled after ${f.cmd.split(":")[1]}s of silence`); }
    else if (f.cmd === "birebir-access") { fetchDms(); }
    else if (f.cmd === "room-closed") {
      const left = state.room, what = isDm(left) ? "this conversation" : "this loca";
      leaveFor(left, locaToReturnTo(left), where => `${what} was closed — ${where}`);
    }
    else { addSys("control: /" + f.cmd); scrollFeed(); }
  }
  else if (f.t === "note") { onNoteFrame(f.note); }
  else if (f.t === "memory") { onMemoryFrame(f); }
  else if (f.t === "notewarn") {
    addSys(`⚠ ${f.by} edited note "${f.key}" but can_write = [${f.can_write.join(", ")}]`);
    if (state.tab === "chat") scrollFeed();
  }
  else if (f.t === "mode") { state.mode = f.mode; renderMode(); }
  else if (f.t === "settings") {
    // A new lead changes how every message by that name reads, so the
    // transcript has to be redrawn — the title is worn, not announced once.
    const leadChanged = state.settings?.lead !== f.settings?.lead;
    // A new title is the loca's name in the sidebar entry too.
    const titleChanged = (state.settings?.title || "") !== (f.settings?.title || "");
    state.settings = f.settings;
    markLocaContextReady("settings");
    renderSettings();
    if (leadChanged) { repaintFeed(); renderMembers(); fetchProfile(); }
    if (titleChanged) renderRooms();
    // Who may declare and assign tasks follows the Lead (board.js).
    if (state.tab === "tasks" || state.tab === "board") renderTasks();
  }
  else if (f.t === "typing") { onTyping(f.name, f.on); }
  else if (f.t === "task") {
    state.tasks[f.task.id] = f.task;
    refreshTaskView(f.task);
    // The board, and the counts Focus shows of it.
    renderTasks();
    if (state.tab !== "board") $("boardDot").classList.add("on");
    refreshTaskBadges();
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
    // Directed attention / actionable reminder -> native notification (kind
    // only, no body). Reactions and ordinary messages never reach here. An
    // "Everyone" reminder fans out to a per-member attention each; key the
    // notification by the shared `group` (generation) so the whole fan-out
    // raises ONE native notification, not one per member.
    notifyDesktop(
      isReminder ? "reminder" : "attention",
      null,
      f.attention.room || state.room,
      f.attention.group || f.attention.id,
    );
    soundForAttention(f.attention);
    if (isReminder) rebuildReminderChatProjection();
    renderReminderSettings();
    renderReminderHistory();
  }
  else if (f.t === "wait") {
    if (f.wait) state.waits[f.waiter] = f.wait; else delete state.waits[f.waiter];
    refreshContinuityTab();
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
  else if (f.t === "vault") { onVaultFrame(f); }
  else if (f.t === "mod") { state.mod = f.state; renderMembers(); }
  else if (f.t === "evicted") { onEvicted(f, from); }
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
  // Kick and ban were asked in words under the seat first (roster.js).
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
  $("leadKey").classList.toggle("hidden", !isLocaOperator() || !state.room);
  select.classList.toggle("hidden", !isLocaOperator() || !state.room);
  if (!isLocaOperator() || !state.room) return;

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
  select.dataset.tip = current ? `${current} is lead` : "No lead yet";
  select.dataset.tipDetail = current
    ? "Choose another name to transfer, or no lead to end."
    : "Choose who leads this loca.";
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
