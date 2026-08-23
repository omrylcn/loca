"use strict";
// Health, mentions, search, message delivery, connection, and room lifecycle.
let healthTimer = null;
function startHealthPoll() {
  if (healthTimer) clearInterval(healthTimer);
  healthTimer = setInterval(pollHealth, 5000);
  pollHealth();
}
async function pollHealth() {
  try {
    const r = await fetch(serverBase() + "/health");
    const h = await r.json();
    state.homeRoom = h.loca_agent_room || state.homeRoom;
    state.locaAgents = h.loca_agents || (h.loca_agent ? [h.loca_agent] : []);
    refreshPeopleRuntime();
    // Dev mode (no ADMIN_TOKEN on the server) means admin controls are open.
    if (state.adminOpen !== h.admin_open) {
      state.adminOpen = h.admin_open;
      if (!h.needs_token) setLocked(false);   // open house (dev): no door
      $("adminbar").classList.toggle("on", isAdmin());
      $("adminToggle").classList.toggle("hidden", !isAdmin());
      $("liveToggle").classList.toggle("hidden", !isAdmin());
      $("stopBtn").classList.toggle("hidden", !isAdmin());
      $("tabPeople").classList.toggle("hidden", !isAdmin());
      fetchSeated();   // admin just appeared: seats we could not see before
      fetchLobby();
      renderMembers();
      renderSettings();
    }
    if (state.epoch === null) { state.epoch = h.epoch; return; }
    if (h.epoch !== state.epoch) {
      // A new server build is live. Don't resync stale JS against it —
      // reload the page so the operator ALWAYS runs the current UI.
      // (Guarded so a flapping server can't cause a reload storm.)
      const lastReload = Number(sessionStorage.getItem("loca-reloaded") || 0);
      if (Date.now() - lastReload > 10000) {
        sessionStorage.setItem("loca-reloaded", String(Date.now()));
        addSys("the table was reset — picking up the new build…");
        setTimeout(() => location.reload(), 600);
      } else {
        state.epoch = h.epoch;
        if (state.room) joinRoom(state.room);
      }
    }
    if (!document.body.classList.contains("locked")) refreshRooms();
  } catch (e) { /* server down; WS onclose already retries */ }
}

/* ---- @mention autocomplete ---- */
let mentionSel = 0, mentionMatches = [];
function updateMentionPop() {
  const input = $("msg");
  const val = input.value.slice(0, input.selectionStart);
  const m = val.match(/@([\w-]*)$/);            // @ + word right before cursor
  const pop = $("mentionPop");
  if (!m) { pop.classList.add("hidden"); mentionMatches = []; return; }
  const q = m[1].toLowerCase();
  // The loca agent keeps no seat in most locas, but you must always be able
  // to call it — so it's offered here even when it isn't in the roster.
  const names = state.members.map(x => x.name);
  const cands = ["all", ...names.filter(n => n !== state.name)];
  for (const agent of state.locaAgents) {
    if (agent !== state.name && !names.includes(agent) && !cands.includes(agent)) {
      cands.push(agent);
    }
  }
  mentionMatches = cands.filter(n => n.toLowerCase().startsWith(q)).slice(0, 6);
  if (!mentionMatches.length) { pop.classList.add("hidden"); return; }
  mentionSel = 0;
  pop.innerHTML = mentionMatches.map((n, i) => {
    const mem = state.members.find(x => x.name === n);
    const tag = n === "all" ? "everyone"
      : (mem ? mem.type : (state.locaAgents.includes(n) ? "loca agent" : ""));
    return `<div data-i="${i}" class="${i === 0 ? "sel" : ""}"><b>@${esc(n)}</b><span class="mtag">${tag}</span></div>`;
  }).join("");
  pop.classList.remove("hidden");
}
function pickMention(name) {
  const input = $("msg");
  const start = input.selectionStart;
  const before = input.value.slice(0, start).replace(/@[\w-]*$/, "@" + name + " ");
  const after = input.value.slice(start);
  input.value = before + after;
  input.selectionStart = input.selectionEnd = before.length;
  $("mentionPop").classList.add("hidden");
  input.focus();
}

/* ---- reply ---- */
function startReply(id) {
  const m = msgById.get(Number(id));
  if (!m) return;
  state.replyTo = Number(id);
  $("replyText").textContent = `↩ replying to ${m.sender}: ${m.text.slice(0, 60)}`;
  $("replybar").classList.remove("hidden");
  $("msg").focus();
}
function clearReply() {
  state.replyTo = null;
  $("replybar").classList.add("hidden");
}
function gotoMsg(id) {
  const el = document.querySelector(`.row[data-id="${id}"]`);
  if (el) { el.scrollIntoView({ block: "center" }); el.querySelector(".bubble").classList.add("flash"); setTimeout(() => el.querySelector(".bubble")?.classList.remove("flash"), 1000); }
}

/* ---- rooms create ---- */
async function createRoom() {
  const name = $("newRoomName").value.trim();
  if (!name) return;
  $("newRoomName").value = "";
  joinRoom(name);   // joining a non-existent room creates it server-side
}

let sendInFlight = false;
// An ambiguous network failure does not mean the server rejected the word:
// the POST may have landed and only its response may have been lost. Keep the
// operation id with the restored draft so Enter retries the SAME operation
// instead of creating a duplicate with a fresh UUID.
let restoredSendOperation = null;
function setSendInFlight(on) {
  sendInFlight = on;
  $("sendBtn").disabled = on;
  $("sendBtn").textContent = on ? "sending…" : "send";
  $("composer").classList.toggle("sending", on);
}
function restoreSendDraft(room, payload, error) {
  if (state.room !== room) {
    alert(`${room}: ${error}`);
    return;
  }
  const input = $("msg");
  const typedWhileSending = input.value.trim();
  if (typedWhileSending) {
    input.value = `${payload.text}\n${input.value}`;
    // The combined text is a new utterance, so it must not inherit the old
    // operation id.
    restoredSendOperation = null;
  } else {
    input.value = payload.text;
    $("target").value = payload.target || "";
    if (payload.reply_to) startReply(payload.reply_to);
    restoredSendOperation = { room, payload: { ...payload } };
  }
  autoGrow();
  addSys(error);
  input.focus();
}
function sameRestoredOperation(room, text, replyTo, target) {
  const restored = restoredSendOperation;
  const p = restored?.payload;
  return restored?.room === room
    && p?.text === text
    && (p?.reply_to ?? null) === (replyTo ?? null)
    && (p?.target ?? null) === (target ?? null);
}
function postMessageRequest(room, payload) {
  return fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/messages`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(payload),
  });
}
async function postMessageWithSafeRetry(room, payload) {
  try {
    const first = await postMessageRequest(room, payload);
    // Reverse proxies can lose the upstream response after the server has
    // committed. One retry is safe because it carries the same op_id.
    if (![502, 503, 504].includes(first.status)) return first;
  } catch (e) {
    // A broken connection is equally ambiguous: retry the same operation once.
  }
  await new Promise(resolve => setTimeout(resolve, 350));
  return postMessageRequest(room, payload);
}
async function acceptPostedMessage(response, room) {
  const posted = await response.json().catch(() => null);
  if (!posted?.id) return;
  state.roomLatest[room] = Math.max(Number(state.roomLatest[room] || 0), Number(posted.id));
  // The POST response is the acknowledgement. Render it immediately instead
  // of making the writer wait for a delayed WS echo. addMsg's id set makes the
  // later echo harmless.
  if (state.room === room) onFrame({ t: "msg", message: posted });
}

async function send() {
  const input = $("msg"); const text = input.value.trim();
  if (!text || !state.room || sendInFlight) return;
  const leadCommand = parseLeadCommand(text);
  if (leadCommand) {
    if (!isAdmin()) {
      addSys("only the loca operator can name or end the lead");
      return;
    }
    if (leadCommand.error) {
      addSys(leadCommand.error);
      return;
    }
    setSendInFlight(true);
    try {
      if (await setLead(leadCommand.lead)) {
        input.value = "";
        autoGrow();
      }
    } catch (e) {
      addSys("could not change lead — connection failed");
    } finally {
      setSendInFlight(false);
    }
    return;
  }
  const goalCommand = parseGoalCommand(text);
  if (goalCommand) {
    if (!isLocaOperator()) {
      addSys("only a loca operator can set or remove the room goal");
      return;
    }
    if (goalCommand.error) {
      addSys(goalCommand.error);
      return;
    }
    setSendInFlight(true);
    try {
      if (await applyGoalCommand(goalCommand.outcome)) {
        input.value = "";
        autoGrow();
      }
    } catch (e) {
      addSys("could not change the room goal — connection failed");
    } finally {
      setSendInFlight(false);
    }
    return;
  }
  // A message that is nothing but a mention wakes everybody to say nothing.
  // Easy to send by accident: the first Enter picks the name from the
  // autocomplete, the second fires before you have typed the sentence.
  if (/^@[\w-]+$/.test(text)) {
    addSys(`"${text}" tek başına — ne söyleyeceğini yaz, sonra gönder`);
    return;
  }
  const room = state.room;
  const replyTo = state.replyTo;
  const target = $("target").value || null;
  const opId = sameRestoredOperation(room, text, replyTo, target)
    ? restoredSendOperation.payload.op_id
    : (globalThis.crypto?.randomUUID?.()
      || `web-${Date.now()}-${Math.random().toString(16).slice(2)}`);
  const payload = {
    sender: state.name, sender_type: "user", target, text,
    reply_to: replyTo, op_id: opId,
  };
  restoredSendOperation = null;
  // Give immediate feedback and make repeated Enter presses harmless while
  // this exact operation is unresolved. The stable op_id below also makes the
  // one authorized retry idempotent at the server.
  input.value = "";
  autoGrow();
  clearReply();
  setSendInFlight(true);
  try {
    // Admin token (if set) is sent so the admin can bypass mode gating.
    const r = await postMessageWithSafeRetry(room, payload);
    if (r.status === 401) {
      // A davet session may have died with a restart; renew and retry once.
      await takeSession();
      if (state.session) {
        const again = await postMessageWithSafeRetry(room, payload);
        if (again.ok) { await acceptPostedMessage(again, room); return; }
      }
      restoreSendDraft(room, payload, "unauthorized — check the loca key");
      return;
    }
    if (r.status === 403) { restoreSendDraft(room, payload, "blocked: " + (await r.text())); return; }
    if (r.status === 429) { restoreSendDraft(room, payload, "rate limited: " + (await r.text())); return; }
    if (!r.ok) { restoreSendDraft(room, payload, `send failed (${r.status}): ` + (await r.text())); return; }
    await acceptPostedMessage(r, room);
  } catch (e) {
    restoreSendDraft(
      room,
      payload,
      "send uncertain — message restored; retry is safe and will not duplicate",
    );
  } finally {
    setSendInFlight(false);
  }
}

// Connecting = applying the identity inputs + joining a room. Room clicks and
// the connect button share this, so joining always uses what's typed in the
// sidebar (no more "joined as the default name before pressing connect").
async function doConnect(room) {
  setMobileSidebar(false);
  const nextName = $("name").value.trim() || "operator";
  const nextPairing = $("pairingCode").value.trim();
  const nextRoomToken = $("roomToken").value.trim();
  const identityChanged = nextName !== state.name
    || nextRoomToken !== state.roomToken
    || (!!nextPairing && nextPairing !== state.pairing);
  if (identityChanged) {
    resetRoomPreferenceIdentity();
    state.profile = null;
  }
  state.name = nextName;
  state.pairing = nextPairing;
  state.roomToken = nextRoomToken;
  state.server = serverBase();
  // A private server binds identity to a session; the browser must take one
  // too, or the WS handshake is refused and you sit at a table you can't
  // speak at. Cheap and harmless when the server doesn't require it.
  await takeSession();
  startHealthPoll();
  $("adminbar").classList.toggle("on", isAdmin());
  $("adminToggle").classList.toggle("hidden", !isAdmin());
  $("liveToggle").classList.toggle("hidden", !isAdmin());
  $("tabPeople").classList.toggle("hidden", !isAdmin());
  $("adminNote").textContent = isAdmin() ? "Admin access · changes apply only to this loca." : "";
  refreshRooms();
  joinRoom(room || state.room || state.homeRoom);
  // /stop is a control broadcast — the server drops it without admin
  // authority, so only show the button when it would actually work.
  $("stopBtn").classList.toggle("hidden", !isAdmin());
  $("connectBtn").className = ""; $("connectBtn").textContent = "retake your seat";
  // Collapse the settings box and show a one-line identity instead, so the
  // room list and ONLINE panel get the sidebar space.
  setConnOpen(false);
  await fetchProfile();
  $("leaveBtn").classList.remove("hidden");
  // Remember the seat: an auto-reloaded page retakes it without a click. The
  // The one-use master pairing code is deliberately NOT stored. After a reload
  // the master takes the next code from the server terminal; the root key
  // remains in the building's .env throughout.
  try { localStorage.setItem("loca-seat", JSON.stringify({
    name: state.name, roomToken: state.roomToken, room: state.room,
  })); } catch (e) {}
}
$("connectBtn").onclick = () => { doConnect(state.room || state.homeRoom); };

// Close/reopen a room (archive): read-only but everything is kept. Reversible,
// and a room must be closed before it can be deleted.
async function archiveRoom(room, on) {
  if (on && !confirm(`Close "${room}"? It becomes read-only — history and notes are kept, and you can reopen it.`)) return;
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/settings`, {
    method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ archived: on }),
  });
  if (r.status === 401) { alert("admin token required"); return; }
  refreshRooms();
  if (room === state.room) fetchSettings();
}

// Seal a room for good. Guarded: only archived rooms, and you must type the
// room name. The loca never reopens; its history stays as an audit record.
async function deleteRoom(room) {
  const typed = prompt(`PERMANENTLY SEAL "${room}"?\n\nThe loca closes forever and cannot be reopened. Messages, notes, tasks and journal stay in the record.\nType the room name to confirm:`);
  if (typed !== room) { if (typed !== null) alert("Name didn't match — nothing sealed."); return; }
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}`, {
    method: "DELETE", headers: adminHeaders({}),
  });
  if (r.status === 401) { alert("admin token required"); return; }
  if (r.status === 409) { alert("Close (🔒) the room first, then delete it."); return; }
  if (state.room === room) doConnect(state.homeRoom);
  refreshRooms();
}
