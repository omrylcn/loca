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
      $("liveToggle").classList.toggle("hidden", !isAdmin() || !extensionOn("live"));
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
    retryOwedRenewal();
    if (!document.body.classList.contains("locked")) refreshRooms();
    // Notifications ride the same tick (notifications.js, loaded later).
    if (typeof pollNotifications === "function") pollNotifications();
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
  // A private conversation reaches one person: `@all` cannot widen it and
  // nothing in it is carried to the loca agent, so neither is offered.
  if (isDm(state.room)) {
    const peer = dmOf(state.room)?.peer;
    cands.splice(0, cands.length, ...(peer && peer !== state.name ? [peer] : []));
  }
  for (const agent of isDm(state.room) ? [] : state.locaAgents) {
    if (agent !== state.name && !names.includes(agent) && !cands.includes(agent)) {
      cands.push(agent);
    }
  }
  // A full name is never an address: the completion inserts the @name,
  // and a full name only helps find it (any of its words, accents aside).
  const fold = text => String(text).normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  const byFullName = n => q && fold(personCardFor(n)?.full_name || "").split(" ").some(w => w.startsWith(fold(q)));
  mentionMatches = cands.filter(n => n.toLowerCase().startsWith(q) || byFullName(n)).slice(0, 6);
  if (!mentionMatches.length) { pop.classList.add("hidden"); return; }
  mentionSel = 0;
  pop.innerHTML = mentionMatches.map((n, i) => {
    const mem = state.members.find(x => x.name === n);
    const tag = n === "all" ? "everyone"
      : (mem ? mem.type : (state.locaAgents.includes(n) ? "loca agent" : ""));
    const full = personCardFor(n)?.full_name;
    return `<div data-i="${i}" class="${i === 0 ? "sel" : ""}"><b>@${esc(n)}</b>${full ? ` <bdi class="fullname">${esc(full)}</bdi>` : ""}<span class="mtag">${tag}</span></div>`;
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
// `+` opens the create screen (extensions.js): the name typed here carries
// over, and the loca is created there with the extensions ticked.
function createRoom() {
  openCreateLoca($("newRoomName").value.trim());
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
// `newOperation`: the server refused this operation id for good (409), so the
// same words sent again must go out as a new operation. The draft, its target
// and its reply link come back as usual; only the operation id is not kept.
function restoreSendDraft(room, payload, error, { newOperation = false } = {}) {
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
    restoredSendOperation = newOperation ? null : { room, payload: { ...payload } };
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
// The session each message request went out with: a 401 is about that one,
// not about whichever session the page holds when the answer arrives.
const messageSentWith = new WeakMap();
function postMessageRequest(room, payload) {
  const headers = adminHeaders({ "content-type": "application/json" });
  return fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/messages`, {
    method: "POST", headers, body: JSON.stringify(payload),
  }).then(response => {
    messageSentWith.set(response, headers["x-session-token"] || null);
    return response;
  });
}
// The retry is the writer's own, like the message. postMessageRequest takes
// its credentials from whoever is signed in when it runs, so a retry made
// after the writer left would post their message under the next person's
// session. The generation is checked before every request and after every
// wait; overtaken, this throws the AbortError a cancelled request gives and
// sends nothing.
function overtakenSend() {
  return new DOMException("the writer signed out before this was sent", "AbortError");
}
async function postMessageWithSafeRetry(room, payload, gen = state.buildingGen) {
  if (buildingStale(gen)) throw overtakenSend();
  try {
    const first = await postMessageRequest(room, payload);
    if (buildingStale(gen)) throw overtakenSend();
    // Reverse proxies can lose the upstream response after the server has
    // committed. One retry is safe because it carries the same op_id.
    if (![502, 503, 504].includes(first.status)) return first;
  } catch (e) {
    if (buildingStale(gen)) throw overtakenSend();
    // A broken connection is equally ambiguous: retry the same operation once.
  }
  await new Promise(resolve => setTimeout(resolve, 350));
  if (buildingStale(gen)) throw overtakenSend();
  return postMessageRequest(room, payload);
}
async function acceptPostedMessage(response, room, gen = state.buildingGen) {
  const posted = await response.json().catch(() => null);
  if (!posted?.id || buildingStale(gen)) return;
  state.roomLatest[room] = Math.max(Number(state.roomLatest[room] || 0), Number(posted.id));
  // The POST response is the acknowledgement. Render it immediately instead
  // of making the writer wait for a delayed WS echo. addMsg's id set makes the
  // later echo harmless.
  if (state.room === room) onFrame({ t: "msg", message: posted });
}

/* ---- attachments (composer) ---- */
const ATTACH_MAX_BYTES = 10 * 1024 * 1024;
const ATTACH_MAX_COUNT = 4;

function fmtBytes(n) {
  if (n == null) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

// Upload one file to the current room's attachment store; returns the stored
// ref {id, sha256, name, mime, size} or null. The server sniffs+validates the
// type, so a wrong-typed or oversize file is rejected here (not on send).
async function uploadAttachment(file) {
  if (!state.room) return null;
  const headers = adminHeaders({ "x-filename": file.name || "attachment" });
  // Let the browser's Blob type ride as content-type; the server matches it to
  // the sniff. An empty type means "no claim" and the server sniffs freely.
  const r = await fetch(
    `${serverBase()}/rooms/${encodeURIComponent(state.room)}/attachments`,
    { method: "POST", headers, body: file },
  );
  if (!r.ok) {
    addSys(`attach failed (${r.status}): ${(await r.text()).slice(0, 120)}`);
    return null;
  }
  return await r.json();
}

// Stage a list of chosen files: validate count/size client-side, upload each,
// and add the returned refs to the pending strip. Duplicate ids are ignored.
async function stageAttachments(files) {
  if (!state.room) { addSys("join a loca before attaching a file"); return; }
  if (isWatchingDoor()) { addSys("you are watching from the door — take a seat to attach a file"); return; }
  // The files are their chooser's: once that person has signed out, the rest
  // are not uploaded and none lands in the next person's composer.
  const gen = state.buildingGen;
  for (const file of files) {
    if (buildingStale(gen)) return;
    if (state.pendingAttachments.length >= ATTACH_MAX_COUNT) {
      addSys(`at most ${ATTACH_MAX_COUNT} attachments per message`);
      break;
    }
    if (file.size > ATTACH_MAX_BYTES) {
      addSys(`"${file.name}" is larger than 10 MB`);
      continue;
    }
    const ref = await uploadAttachment(file);
    if (buildingStale(gen)) return;
    if (ref && !state.pendingAttachments.some(a => a.id === ref.id)) {
      state.pendingAttachments.push(ref);
      renderAttachStrip();
    }
  }
}

function removePendingAttachment(id) {
  state.pendingAttachments = state.pendingAttachments.filter(a => a.id !== id);
  renderAttachStrip();
}

function clearPendingAttachments() {
  state.pendingAttachments = [];
  renderAttachStrip();
}

// Draw the staged-file chips above the composer (name + size + remove).
function renderAttachStrip() {
  const strip = $("attachStrip");
  if (!strip) return;
  strip.innerHTML = "";
  if (!state.pendingAttachments.length) { strip.classList.add("hidden"); return; }
  strip.classList.remove("hidden");
  for (const a of state.pendingAttachments) {
    const chip = document.createElement("span");
    chip.className = "pendingchip";
    const isImage = (a.mime || "").startsWith("image/");
    chip.innerHTML =
      `<span class="pglyph">${isImage ? "🖼" : "📎"}</span>` +
      `<span class="pname">${esc(a.name || a.id.slice(0, 8))}</span>` +
      `<span class="psize">${fmtBytes(a.size)}</span>`;
    const x = document.createElement("button");
    x.type = "button";
    x.className = "premove";
    x.setAttribute("aria-label", `Remove ${a.name || "attachment"}`);
    x.textContent = "✕";
    x.addEventListener("click", () => removePendingAttachment(a.id));
    chip.appendChild(x);
    strip.appendChild(chip);
  }
}

async function send() {
  const input = $("msg"); const text = input.value.trim();
  const hasAttachments = state.pendingAttachments.length > 0;
  if ((!text && !hasAttachments) || !state.room || sendInFlight) return;
  if (isWatchingDoor()) {
    addSys("you are watching from the door — take a seat to speak");
    return;
  }
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
    const gen = state.buildingGen;
    try {
      // The composer may be the next person's by the time this answers.
      if (await setLead(leadCommand.lead) && !buildingStale(gen)) {
        input.value = "";
        autoGrow();
      }
    } catch (e) {
      if (!buildingStale(gen)) addSys("could not change lead — connection failed");
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
    const gen = state.buildingGen;
    try {
      if (await applyGoalCommand(goalCommand.outcome) && !buildingStale(gen)) {
        input.value = "";
        autoGrow();
      }
    } catch (e) {
      if (!buildingStale(gen)) addSys("could not change the room goal — connection failed");
    } finally {
      setSendInFlight(false);
    }
    return;
  }
  // A message that is nothing but a mention wakes everybody to say nothing.
  // Easy to send by accident: the first Enter picks the name from the
  // autocomplete, the second fires before you have typed the sentence.
  if (/^@[\w-]+$/.test(text) && !hasAttachments) {
    addSys(`"${text}" on its own — write what you want to say, then send`);
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
  if (hasAttachments) {
    payload.attachments = state.pendingAttachments.map(a => a.id);
  }
  restoredSendOperation = null;
  // Give immediate feedback and make repeated Enter presses harmless while
  // this exact operation is unresolved. The stable op_id below also makes the
  // one authorized retry idempotent at the server.
  input.value = "";
  autoGrow();
  clearReply();
  setSendInFlight(true);
  // This message is its writer's. Once that person has signed out, nothing
  // here goes on for whoever sits down next: it is not sent again under their
  // session, and it is not put back into their composer. Checked after every
  // wait, because the sign-out can fall inside any of them.
  const gen = state.buildingGen;
  const refuse = async (label, response) => {
    const why = await response.text();
    if (!buildingStale(gen)) restoreSendDraft(room, payload, label + why);
  };
  // What a refused post means, whichever attempt it was (the first, or the
  // one retried under a renewed session). False when the response is not a
  // refusal handled here: it succeeded, or it is a 401.
  const settleRefusal = async (response) => {
    if (response.status === 403) { await refuse("blocked: ", response); return true; }
    if (response.status === 429) { await refuse("rate limited: ", response); return true; }
    if (response.status === 409) {
      // The server cannot tell whether an operation with this id, stored
      // before an upgrade, was this person's. Retrying the same id would be
      // refused for ever, and nothing is sent again by itself: the words go
      // back to the composer, and sending them again is the person's choice
      // and a new operation.
      const why = await response.text();
      if (!buildingStale(gen)) {
        restoreSendDraft(
          room, payload,
          `not sent: ${why}. Check the conversation: it may already be there. `
            + "Send again to post it as a new message.",
          { newOperation: true },
        );
      }
      return true;
    }
    if (!response.ok && response.status !== 401) {
      await refuse(`send failed (${response.status}): `, response);
      return true;
    }
    return false;
  };
  try {
    // Admin token (if set) is sent so the admin can bypass mode gating.
    const r = await postMessageWithSafeRetry(room, payload, gen);
    if (buildingStale(gen)) return;
    if (r.status === 401) {
      // An invite session may have died with a restart; renew and retry once.
      // The 401 is about the session the message went out with. Renew only
      // when that is still the page's session, or the page has none and is
      // not taking one: a session taken since (taking one's seat again) is
      // used, not replaced, and one still being taken is waited for, not
      // raced. Replacing it would leave it valid on the server, held by
      // nobody. The retry carries the same op_id either way.
      const refused = messageSentWith.get(r) ?? null;
      // The server answers 401 both when a session is no longer valid and
      // when a valid session may not enter this loca (its invite revoked, a
      // kick). Only the first is cured by a new session, and the server says
      // which: a session it no longer accepts is turned away before any loca
      // is looked at ("davet required", routes/access.rs require_membership),
      // while "davet required for this loca" comes from the loca's own door
      // (RoomAccess), which a request reaches only with a session the server
      // accepted. That session is not replaced: the person is told the truth.
      if (refused && refused === state.session
          && (await r.text().catch(() => "")).trim() === "davet required for this loca") {
        if (buildingStale(gen)) return;
        restoreSendDraft(room, payload, "you are no longer in this loca — you are still signed in");
        return;
      }
      if (sessionTaking) {
        if (!(await sessionTaking.promise) || buildingStale(gen)) return;
      } else if (!state.session || state.session === refused) {
        // Replaced, it is ended (the server has just said it is gone; this
        // only makes sure).
        endUnheldSession(state.server || serverBase(), state.session);
        if (!(await takeSession()) || buildingStale(gen)) return;
      }
      if (state.session && state.session !== refused) {
        const again = await postMessageWithSafeRetry(room, payload, gen);
        if (buildingStale(gen)) return;
        if (again.ok) {
          await acceptPostedMessage(again, room, gen);
          // The attachments waiting now may be the next person's.
          if (!buildingStale(gen)) clearPendingAttachments();
          return;
        }
        // The retry was answered in its own right: say what it said, not
        // "unauthorized".
        if (await settleRefusal(again)) return;
      }
      restoreSendDraft(room, payload, "unauthorized — check the loca key");
      return;
    }
    if (await settleRefusal(r)) return;
    await acceptPostedMessage(r, room, gen);
    if (buildingStale(gen)) return;
    clearPendingAttachments();
  } catch (e) {
    if (buildingStale(gen)) return;
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
  if (hostWithoutServer()) { showHostWithoutServer(); return; }
  // A connect is the latest word on who is here. An automatic retry still
  // waiting from an earlier failed start would re-read the remembered seat
  // and could put that identity back over this one.
  if (typeof authRetryTimer !== "undefined" && authRetryTimer) { clearTimeout(authRetryTimer); authRetryTimer = null; }
  const typedName = $("name").value.trim();
  const nextName = typedName || "operator";
  const nextPairing = $("pairingCode").value.trim();
  const nextRoomToken = $("roomToken").value.trim();
  const identityChanged = nextName !== state.name
    || nextRoomToken !== state.roomToken
    || (!!nextPairing && nextPairing !== state.pairing)
    || (!!state.server && serverBase() !== state.server);
  if (identityChanged) {
    // Someone else, or another server, over a live session: nothing of the
    // last one's Building stays, and its answers still out are dropped.
    // The generation moves on in every case, so a session still being taken
    // or renewed for the last identity is answered into nothing. (Its status
    // reads "unknown" while that is out, so the status cannot decide this.)
    // Only the first connect of a page load skips the clearing itself: it
    // runs before the Building scripts below this one have loaded, and there
    // is nothing of anyone's to clear yet.
    if (document.readyState === "loading") moveBuildingOn();
    else clearBuildingData();
    resetRoomPreferenceIdentity();
    state.profile = null;
  }
  // Exchanging a credential (below) drops the session this page holds:
  // takeSessionFor starts by forgetting it. Dropped, nobody holds it, unless
  // it is the Master session this browser keeps, so it is ended at the server
  // it came from, taken here before state.server is overwritten.
  if (state.session && (nextRoomToken || nextPairing) && !storedAdminSession(state.session)) {
    endUnheldSession(state.server || serverBase(), state.session);
  }
  state.name = nextName;
  state.pairing = nextPairing;
  state.roomToken = nextRoomToken;
  state.server = serverBase();
  // A private server binds identity to a session; the browser must take one
  // too, or the WS handshake is refused and you sit at a table you can't
  // speak at. Cheap and harmless when the server doesn't require it.
  // Overtaken by a sign-out or another sign-in while the session was being
  // taken: this connect was for someone who is no longer here, and the rest
  // of it (the seat, the room, the remembered identity) is not theirs to set.
  const gen = state.buildingGen;
  if (!(await takeSession()) || buildingStale(gen)) return;
  if (state.authStatus === "unknown") {
    showAuthUnavailable();
    return;
  }
  document.body.classList.remove("auth-pending");
  // A Smaster's key names it; the name typed at sign-in is the name it
  // wants to be called. Apply it as a profile rename before taking a seat,
  // so it is the name the table sees. Empty keeps the current name.
  let renameRefused = "";
  if (state.entryPath === "smaster" && state.adminSession && typedName && typedName !== state.name) {
    renameRefused = await applyChosenName(typedName);
    if (buildingStale(gen)) return;
  }
  startHealthPoll();
  $("adminbar").classList.toggle("on", isAdmin());
  $("adminToggle").classList.toggle("hidden", !isAdmin());
  $("liveToggle").classList.toggle("hidden", !isAdmin() || !extensionOn("live"));
  $("tabPeople").classList.toggle("hidden", !isAdmin());
  $("adminNote").textContent = isAdmin() ? "Admin access · changes apply only to this loca." : "";
  refreshRooms();
  await land(gen, room || state.room || state.homeRoom);
  if (buildingStale(gen)) return;
  // /stop is a control broadcast — the server drops it without admin
  // authority, so only show the button when it would actually work.
  $("stopBtn").classList.toggle("hidden", !isAdmin());
  $("connectBtn").className = ""; $("connectBtn").textContent = "retake your seat";
  // Put the sign-in form away and show the account menu instead, so the
  // room list and ONLINE panel get the sidebar space.
  setConnOpen(false);
  await fetchProfile();
  if (buildingStale(gen)) return;
  if (renameRefused) showRenameRefusal(typedName, renameRefused);
  // Remember the seat: an auto-reloaded page retakes it without a click. The
  // The one-use master pairing code is deliberately NOT stored. After a reload
  // the master takes the next code from the server terminal; the root key
  // remains in the building's .env throughout.
  try { localStorage.setItem("loca-seat", JSON.stringify({
    name: state.name, roomToken: state.roomToken, room: state.room || landingPending,
  })); } catch (e) {}
  // The seat is taken and (if the browser allowed it) remembered. A join
  // request that handed this key over waits for exactly this before it
  // tells the server the key arrived (joinrequest.js).
  document.dispatchEvent(new CustomEvent("loca:seat-taken", { detail: { gen } }));
}
$("connectBtn").onclick = () => { doConnect(state.room || state.homeRoom); };

// Where a connect lands. The loca asked for (the remembered seat: the last
// loca this person was in, rememberLastLoca; else Care, which is the ranks'
// own) when this person may enter it and it is open; else Care if they may
// enter it; else the first open loca it has a seat in; else none: it is in
// the Lobby. GET /rooms lists exactly the locas this person may enter, by the
// same door decision as the socket, so a member is never sent to knock at a
// door that will not open (Care is the ranks' and caretakers' only: PRINCIPLES
// "Care"). A closed loca is kept, read-only, and nobody is sent back into it
// while another is open.
//
// If the list cannot be read (the request fails, the server answers an error,
// or the answer is not a list) the page does not know which doors open for
// this person. One door is known without the list: Care opens for every rank.
// Any other loca is not known to open, for a rank either (a Smaster outside a
// group is refused at that group's locas; a private pair's loca opens for its
// two people only), so the answer is `undefined`: no door is tried, and
// land() reads the list again.
async function landingRoom(wanted) {
  let rooms;
  try {
    const r = await fetch(serverBase() + "/rooms", { headers: adminHeaders({}) });
    rooms = r.ok ? await r.json() : null;
  } catch (e) { rooms = null; }
  if (!Array.isArray(rooms)) return isAdmin() && isRankHome(wanted) ? wanted : undefined;
  const open = room => rooms.some(rm => rm.room === room && !rm.archived);
  if (open(wanted)) return wanted;
  if (open(state.homeRoom)) return state.homeRoom;
  const locas = rooms.filter(rm => !isDm(rm.room));
  const first = orderedSidebarRooms(locas.filter(rm => !rm.archived))[0]
    || orderedSidebarRooms(locas)[0] || locas[0];
  return first ? first.room : null;
}

// The loca a reload, a restart or the next sign-in goes back to is the last
// one this person was in: for everyone (a member, a Smaster, the Master) and
// everywhere (the browser and the desktop, where the seat lives in the OS
// keychain). Only the room of the seat this page remembers for this key
// changes, never who it is; with no remembered seat nothing is written.
// landingRoom still decides whether it may be entered: one released or closed
// since is not used, and the connect that lands elsewhere writes that room
// over it; while the list cannot be read it is kept (landingPending). A
// direct message is a conversation, not a loca to land in: being in one leaves the
// last loca remembered.
function rememberLastLoca(room) {
  if (!room || isDm(room)) return;
  try {
    const seat = JSON.parse(localStorage.getItem("loca-seat") || "null");
    if (!seat || typeof seat !== "object" || seat.room === room) return;
    if ((seat.roomToken || "") !== (state.roomToken || "")) return;
    seat.room = room;
    localStorage.setItem("loca-seat", JSON.stringify(seat));
  } catch (e) { /* storage unavailable: the next load lands as before */ }
}

// Take the landing: a loca, the Lobby, or, when the list of locas could not be
// read, wait and read it again (2 s, then doubling to 30 s) without opening a
// socket. The wait ends when the list is read, when the person opens a loca
// itself, or when the Building moves on (sign-out, another sign-in).
//
// A landing is a decision made for the person, so a later decision replaces
// it: every land() and every joinRoom() (socket.js) moves landingSeq on, and
// an answer that comes back for an earlier one is dropped. Without that, a
// list that answers late would move the person out of the loca it has just
// opened itself.
const LANDING_RETRY_MS = 2000, LANDING_RETRY_MAX_MS = 30000;
let landingTimer = null;
let landingPending = null;   // the loca asked for while the list is unread
let landingSeq = 0;
function cancelLandingRetry() {
  landingSeq++;
  if (landingTimer !== null) clearTimeout(landingTimer);
  landingTimer = null;
  landingPending = null;
}
async function land(gen, wanted, attempt = 0) {
  cancelLandingRetry();
  const seq = landingSeq;
  const landing = await landingRoom(wanted);
  if (buildingStale(gen) || seq !== landingSeq) return;
  takeLanding(gen, wanted, landing, attempt);
}
function takeLanding(gen, wanted, landing, attempt = 0) {
  if (landing) { joinRoom(landing); return; }
  if (landing === null) { showLobbyWait(); return; }
  showLobbyWait(
    "the list of your locas could not be read, so no loca is opened yet — trying again.",
    "could not read your locas — trying again", "no loca open");
  landingPending = wanted;
  const delay = Math.min(LANDING_RETRY_MAX_MS, LANDING_RETRY_MS * 2 ** Math.min(attempt, 8));
  landingTimer = setTimeout(() => {
    landingTimer = null;
    // The person is no longer here, or opened a loca itself (which cancels
    // this timer; the check is for a room set any other way): nothing to land.
    if (buildingStale(gen) || state.room !== null) { landingPending = null; return; }
    land(gen, wanted, attempt + 1);
  }, delay);
}

// The one loca this person was last in (not a private conversation), so that
// leaving a private conversation returns there. It belongs to whoever is
// signed in now: a sign-out or another sign-in starts without it.
let lastLoca = null;   // { gen, room }
function rememberLoca(room) { lastLoca = { gen: state.buildingGen, room }; }
function locaToReturnTo(left) {
  const room = lastLoca && !buildingStale(lastLoca.gen) ? lastLoca.room : null;
  return room && room !== left ? room : state.homeRoom;
}

// Leaving the loca on screen without having picked where to go: it was sealed
// (room-closed), or a private conversation was closed here. Where to go is
// the sign-in decision (landingRoom), never Care by default: a member may not
// enter Care and would wait at a door that never opens. Who is here has not
// changed, so the session is kept: this is not doConnect, which takes a new
// one. `say(where)`, when given, is shown first with a moment to read it.
// It is a landing decision like land(): one made after it (another landing,
// or the person opening a loca, joinRoom) replaces it, and it then moves
// nobody.
async function leaveFor(left, wanted, say) {
  const gen = state.buildingGen;
  cancelLandingRetry();
  const seq = landingSeq;
  const landing = await landingRoom(wanted);
  const replaced = () => buildingStale(gen) || seq !== landingSeq || state.room !== left;
  if (replaced()) return;
  const go = () => {
    if (replaced()) return;
    cancelLandingRetry();
    takeLanding(gen, wanted, landing);
  };
  if (!say) { go(); return; }
  addSys(say(landing ? `moving you to ${roomLabel(landing)}`
    : landing === null ? "moving you to the Lobby"
    : "the list of your locas could not be read, so no loca is opened yet"));
  setTimeout(go, 800);
}

// A member no loca has seated waits in the Lobby (PRINCIPLES: admit → Lobby →
// invite/call → private loca). The Lobby is not a loca: it has no chat and
// the browser opens no socket for it, so there is nothing to connect to. Say
// so, instead of "connecting…" to a loca that will never let it in. A call
// lists its loca in the sidebar on the next room refresh. The same view, with
// its own words, shows while the list of locas could not be read (land()).
function showLobbyWait(
  words = "you are in the Lobby, waiting to be called — no loca has a seat for you yet. When someone calls you in, the loca appears in the list.",
  status = "in the Lobby — waiting to be called",
  title = "Lobby",
) {
  cancelReconnect();
  if (state.ws) { state.ws.onclose = null; state.ws.close(); state.ws = null; }
  state.room = null;
  state.members = [];
  setSidebarView("building");
  renderLocaSidebar();
  renderMembers();
  $("curRoom").textContent = title;
  $("roomAvatar").textContent = title === "Lobby" ? "L" : "…";
  revokeInlineAttachmentUrls();
  state.msgs = []; state.seen = new Set(); state.lastId = 0; lastDayKey = null;
  $("feed").innerHTML = "";
  addSys(words);
  setStatus(status, false);
  $("subStatus").textContent = status.replace(/^in the Lobby — /, "");
}

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
  if (state.room === room) leaveFor(room, state.homeRoom);
  refreshRooms();
}
