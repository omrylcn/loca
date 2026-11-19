"use strict";
// Reply threads. A message that others answered wears "N replies · last at
// HH:MM" under it; clicking it opens the thread beside the chat (in place of
// This Loca on a wide screen, over the chat on a narrower one): the message,
// its replies in order, and a box to answer in it. Nothing new is stored: a
// thread is read from the messages' own reply_to, and an answer from the box
// is an ordinary message sent the composer's way (api.js sendFrom) with
// reply_to set to the thread's first message. Conversation stays
// conversation: a thread is a view of what was said, not a work record.
//
// A reply to a reply belongs to the same thread: the first message of a
// thread is found by following reply_to back as far as the loaded chat goes.
// When it goes further back than that, the thread reads it through the
// loca's message history (GET /rooms/:room/messages?since=), a page at a time.

const THREAD_PAGE = 200;
// The messages read for threads in this loca, beyond the loaded chat, and how
// far each thread's reading has got: { cursor, stop, done, loading, failed }.
const threadCache = { room: null, messages: new Map(), progress: new Map() };
// The thread on screen.
const thread = { room: null, rootId: null };

function threadCacheForRoom() {
  if (threadCache.room === state.room) return;
  threadCache.room = state.room;
  threadCache.messages.clear();
  threadCache.progress.clear();
}

// A message of this loca the page holds: in the loaded chat, or read for a thread.
function threadKnown(id) {
  const n = Number(id);
  if (!n) return null;
  if (state.seen.has(n)) return msgById.get(n) || null;
  return threadCache.room === state.room ? threadCache.messages.get(n) || null : null;
}

// The first message of the thread a reply belongs to: reply_to followed back
// as far as the page knows. Ids only ever go down, so the walk ends.
function threadRootOf(m) {
  let id = Number(m?.reply_to || 0);
  for (let hops = 0; id && hops < 100; hops += 1) {
    const up = Number(threadKnown(id)?.reply_to || 0);
    if (!up || up >= id) break;
    id = up;
  }
  return id || null;
}

// A thread's replies the page knows, oldest first.
function threadReplies(rootId) {
  const root = Number(rootId);
  const replies = new Map();
  const consider = (m) => { if (m?.id && m.reply_to && threadRootOf(m) === root) replies.set(m.id, m); };
  for (const id of state.seen) consider(msgById.get(id));
  if (threadCache.room === state.room) for (const m of threadCache.messages.values()) consider(m);
  return [...replies.values()].sort((a, b) => a.id - b.id);
}

function threadCountText(replies) {
  const n = replies.length;
  const last = replies[n - 1];
  return `${n} ${n === 1 ? "reply" : "replies"}${last?.ts ? ` · last at ${fmtTime(last.ts)}` : ""}`;
}

// The "N replies · last at HH:MM" link under a message on screen.
function renderThreadSummary(rootId) {
  const row = document.querySelector(`#feed .row[data-id="${Number(rootId)}"]`);
  const bubble = row?.querySelector(".bubble");
  if (!bubble) return;
  const replies = threadReplies(rootId);
  let link = bubble.querySelector(".threadlink");
  if (!replies.length) { link?.remove(); return; }
  if (!link) {
    link = document.createElement("button");
    link.type = "button";
    link.className = "threadlink";
    link.dataset.thread = String(Number(rootId));
    bubble.appendChild(link);
  }
  link.textContent = threadCountText(replies);
  link.setAttribute("aria-label", `Open the thread: ${link.textContent}`);
}

// addMsg (chat.js) calls this for every message it draws.
function threadsNoteMessage(m) {
  threadCacheForRoom();
  if (!m.reply_to) return;
  const root = threadRootOf(m);
  if (!root) return;
  renderThreadSummary(root);
  if (thread.rootId === root && thread.room === state.room) renderThread({ toEnd: m.sender === state.name });
}

/* ---- the panel ---- */

function threadOpen() {
  return !!thread.rootId && thread.room === state.room && state.tab === "chat";
}

// Beside the chat when This Loca stands there (locainfo.js), over it otherwise.
function placeThreadPanel() {
  const panel = $("threadPanel");
  if (!panel) return;
  const open = threadOpen();
  const beside = open && document.body.classList.contains("locainfo-on");
  const host = beside ? $("locaInfo") : document.body;
  if (panel.parentElement !== host) host.appendChild(panel);
  panel.classList.toggle("hidden", !open);
  panel.classList.toggle("overlay", open && !beside);
  document.body.classList.toggle("thread-open", open);
  document.body.classList.toggle("thread-beside", beside);
  $("threadBackdrop").classList.toggle("hidden", !open || beside);
}

function oldestLoadedId() {
  let oldest = Infinity;
  for (const id of state.seen) if (id < oldest) oldest = id;
  return oldest;
}

function openThread(rootId) {
  const root = Number(rootId);
  if (!root || !state.room) return;
  threadCacheForRoom();
  const changed = thread.rootId !== root;
  thread.rootId = root;
  thread.room = state.room;
  // Its first message further back than the loaded chat: read from there up
  // to the loaded chat. Everything after a loaded message is loaded already.
  if (!state.seen.has(root) && !threadCache.progress.has(root)) {
    threadCache.progress.set(root, { cursor: root - 1, stop: oldestLoadedId(), done: false, loading: false, failed: false });
  }
  if (changed) $("threadMsg").value = "";
  placeThreadPanel();
  renderThread({ toEnd: true });
  $("threadMsg").focus({ preventScroll: true });
  const progress = threadCache.progress.get(root);
  if (progress && !progress.done && !threadKnown(root)) loadThreadEarlier();
}

function closeThread() {
  const wasOverlay = threadOpen() && $("threadPanel").classList.contains("overlay");
  thread.rootId = null;
  thread.room = null;
  placeThreadPanel();
  if (wasOverlay) $("msg")?.focus({ preventScroll: true });
}

function threadMsgHtml(m, rootId) {
  const kind = m.sender_type === "agent" ? "agent" : "user";
  const nested = m.reply_to && Number(m.reply_to) !== Number(rootId) ? threadKnown(m.reply_to) : null;
  return `<article class="threadmsg${m.id === rootId ? " threadroot" : ""}" data-thread-msg="${Number(m.id)}">
    <span class="threadface" aria-hidden="true">${faceHtml(m.sender, m.sender_type, m.sender_principal)}</span>
    <div class="threadmain">
      <div class="threadline"><span class="sender ${kind}">${esc(m.sender)}</span><span class="time" title="${esc(fmtFull(m.ts))}">${m.ts ? fmtMessageTime(m.ts) : ""}</span></div>
      ${nested ? `<div class="threadquote">↩ ${esc(nested.sender)}: ${esc(String(nested.text || "").slice(0, 70))}</div>` : ""}
      <div class="body markdown chatmarkdown">${renderChatMarkdown(m.text || "")}</div>
      ${m.attachments?.length ? `<div class="attachments" data-attfor></div>` : ""}
    </div>
  </article>`;
}

function renderThread({ toEnd = false } = {}) {
  const panel = $("threadPanel");
  if (!panel || !threadOpen()) return;
  const rootId = thread.rootId;
  const root = threadKnown(rootId);
  const replies = threadReplies(rootId);
  const progress = threadCache.progress.get(rootId);
  const list = $("threadMessages");
  const atEnd = list.scrollHeight - list.scrollTop - list.clientHeight < 80;

  // What is still further back than the page has read.
  const earlier = $("threadEarlier");
  const more = !!progress && !progress.done;
  const note = !root
    ? (more || progress?.failed ? "The start of this thread is further back." : "The start of this thread is not in this loca's history.")
    : "Earlier replies may be further back.";
  earlier.classList.toggle("hidden", !!root && !more);
  $("threadEarlierText").textContent = note + (progress?.failed ? " It could not be loaded." : "");
  const load = $("threadLoadEarlier");
  load.classList.toggle("hidden", !more);
  load.disabled = !!progress?.loading;
  load.textContent = progress?.loading ? "Loading…" : progress?.failed ? "Try again" : "Load earlier";

  $("threadTitle").textContent = root ? `Thread · ${root.sender}` : "Thread";
  $("threadRoot").innerHTML = root ? threadMsgHtml(root, rootId) : "";
  $("threadCount").textContent = replies.length ? threadCountText(replies) : "No replies yet";
  $("threadReplies").innerHTML = replies.map(m => threadMsgHtml(m, rootId)).join("");
  for (const m of root ? [root, ...replies] : replies) {
    if (!m.attachments?.length) continue;
    const el = list.querySelector(`[data-thread-msg="${Number(m.id)}"]`);
    if (el) renderAttachments(el, m);
  }
  $("threadMsg").placeholder = root ? `Reply to ${root.sender}` : "Reply in this thread";
  if (toEnd || atEnd) list.scrollTop = list.scrollHeight;
}

// The quotes in the chat that answer a message read since: say what it said.
function refreshEarlierQuotes() {
  for (const quote of document.querySelectorAll("#feed .quote.earlier[data-thread]")) {
    const q = threadKnown(quote.dataset.thread);
    if (!q) continue;
    quote.classList.remove("earlier");
    quote.textContent = `↩ ${q.sender}: ${String(q.text || "").slice(0, 70)}`;
  }
}

// Read the next page of the loca's history between the thread's first
// message and the loaded chat, keeping what belongs to the thread.
async function loadThreadEarlier() {
  const rootId = thread.rootId;
  const room = state.room;
  const progress = threadCache.progress.get(rootId);
  if (!rootId || !room || !progress || progress.done || progress.loading) return;
  const gen = state.buildingGen;
  progress.loading = true;
  progress.failed = false;
  renderThread();
  try {
    const response = await fetch(
      `${serverBase()}/rooms/${encodeURIComponent(room)}/messages?since=${progress.cursor}&limit=${THREAD_PAGE}`,
      { headers: adminHeaders({}) },
    );
    if (buildingStale(gen) || threadCache.room !== room) return;
    if (!response.ok) throw new Error(`history ${response.status}`);
    const page = await response.json();
    for (const m of Array.isArray(page) ? page : []) {
      const id = Number(m?.id || 0);
      if (!id) continue;
      if (id >= progress.stop) { progress.done = true; break; }
      progress.cursor = id;
      if (id === rootId || (m.reply_to && threadRootOf(m) === rootId)) {
        threadCache.messages.set(id, m);
        if (!msgById.has(id)) msgById.set(id, m);
      }
    }
    if (!Array.isArray(page) || page.length < THREAD_PAGE) progress.done = true;
  } catch (e) {
    if (buildingStale(gen) || threadCache.room !== room) return;
    progress.failed = true;
  } finally {
    progress.loading = false;
  }
  if (buildingStale(gen) || threadCache.room !== room) return;
  refreshEarlierQuotes();
  renderThreadSummary(rootId);
  renderThread({ toEnd: true });
}

function sendThreadReply() {
  if (!threadOpen() || !$("threadMsg").value.trim()) return;
  sendFrom({ input: $("threadMsg"), replyTo: thread.rootId });
}

$("threadClose").addEventListener("click", closeThread);
$("threadBackdrop").addEventListener("click", closeThread);
$("threadLoadEarlier").addEventListener("click", loadThreadEarlier);
$("threadReplyForm").addEventListener("submit", (e) => { e.preventDefault(); sendThreadReply(); });
$("threadMsg").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendThreadReply(); }
});
$("threadPanel").addEventListener("keydown", (e) => {
  if (e.key === "Escape") { e.preventDefault(); closeThread(); }
});
$("feed").addEventListener("click", (e) => {
  const open = e.target.closest("[data-thread]");
  if (open) { e.preventDefault(); openThread(open.dataset.thread); }
});
// Info asks for This Loca: the thread gives the column back.
$("locaInfoBtn")?.addEventListener("click", () => { if (threadOpen()) closeThread(); }, true);
// This Loca coming and going (locainfo.js) moves the panel with it.
new MutationObserver(placeThreadPanel).observe(document.body, { attributes: true, attributeFilter: ["class"] });
