"use strict";
// UI event wiring and application bootstrap.
function setConnOpen(open) {
  $("connBox").style.display = open ? "" : "none";
  $("connGear").textContent = open ? "✕" : "⚙";
}
$("connGear").onclick = () => setConnOpen($("connBox").style.display === "none");
// Leaving the whole Building is materially different from releasing one loca.
// Ask explicitly; an accidental click must not revoke the browser session.
$("leaveBtn").onclick = () => $("leaveConfirm").classList.remove("hidden");
$("leaveNo").onclick = () => $("leaveConfirm").classList.add("hidden");
$("leaveYes").onclick = async () => {
  $("leaveConfirm").classList.add("hidden");
  try { localStorage.removeItem("loca-seat"); } catch (e) {}
  try {
    localStorage.removeItem("loca-admin-session");
    sessionStorage.removeItem("loca-admin-session");
  } catch (e) {}
  if (state.ws) { state.ws.onclose = null; state.ws.close(); state.ws = null; }
  const session = state.session;
  if (session) {
    try {
      await fetch(serverBase() + "/sessions", {
        method: "DELETE",
        headers: { "x-session-token": session },
      });
    } catch (e) { /* local logout still completes if the server is unavailable */ }
  }
  state.session = null; state.roomToken = ""; state.pairing = ""; state.adminSession = false; state.sessionExpires = null; state.room = null;
  resetRoomPreferenceIdentity();
  state.profile = null;
  $("roomToken").value = ""; $("pairingCode").value = "";
  $("leaveBtn").classList.add("hidden");
  setLocked(true);
  $("doorline").innerHTML = "you left loca.<br>the key opens it again.";
};
$("applyMode").onclick = applyMode;
function closeProperties() {
  $("adminbar").classList.remove("open");
  $("adminToggle").textContent = "properties ▾";
  $("adminToggle").setAttribute("aria-expanded", "false");
}
$("adminToggle").onclick = () => {
  const open = $("adminbar").classList.toggle("open");
  $("adminToggle").textContent = open ? "properties ▴" : "properties ▾";
  $("adminToggle").setAttribute("aria-expanded", String(open));
};
document.addEventListener("pointerdown", (event) => {
  if (!$("adminbar").classList.contains("open")) return;
  if ($("adminbar").contains(event.target) || $("adminToggle").contains(event.target)) return;
  closeProperties();
});
document.querySelectorAll("#adminbar details.propertygroup").forEach(group => {
  group.addEventListener("toggle", () => {
    if (!group.open) return;
    document.querySelectorAll("#adminbar details.propertygroup").forEach(other => {
      if (other !== group) other.open = false;
    });
  });
});
$("sideToggle").onclick = () => setMobileSidebar(
  !document.body.classList.contains("sidebar-open"),
);
$("sideBackdrop").onclick = () => setMobileSidebar(false);
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { setMobileSidebar(false); closeProperties(); }
});
$("closeLocaBtn").onclick = () => archiveRoom(state.room, true);
$("reopenLocaBtn").onclick = () => archiveRoom(state.room, false);
$("sealLocaBtn").onclick = () => deleteRoom(state.room);
$("applySettings").onclick = applySettings;
$("liveToggle").onclick = toggleLive;
$("peopleList").onclick = async (e) => {
  const jrNote = (t) => { const n = $("jrNotice"); if (n) n.textContent = t; };
  // Join-request admissions, handled right here in the main app (not the desk).
  const approve = e.target.closest("[data-jr-approve]");
  if (approve) {
    approve.disabled = true;
    const r = await fetch(`${serverBase()}/join-requests/${encodeURIComponent(approve.dataset.jrApprove)}/approve`,
      { method: "POST", headers: adminHeaders({}) });
    if (!r.ok) jrNote(await r.text().catch(() => "could not approve"));
    fetchPeople();
    return;
  }
  const deny = e.target.closest("[data-jr-deny]");
  if (deny) {
    deny.disabled = true;
    const r = await fetch(`${serverBase()}/join-requests/${encodeURIComponent(deny.dataset.jrDeny)}/deny`,
      { method: "POST", headers: adminHeaders({}) });
    if (!r.ok) jrNote(await r.text().catch(() => "could not deny"));
    fetchPeople();
    return;
  }
  const mint = e.target.closest("[data-jr-mint]");
  if (mint) {
    mint.disabled = true;
    const r = await fetch(`${serverBase()}/admission-stock`,
      { method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({ count: Number(mint.dataset.jrMint) || 5 }) });
    if (!r.ok) jrNote(await r.text().catch(() => "could not mint"));
    fetchPeople();
    return;
  }
  const b = e.target.closest("[data-unban]");
  if (!b) return;
  await fetch(`${serverBase()}/rooms/${encodeURIComponent(b.dataset.room)}/moderate`, {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ action: "unban", name: b.dataset.unban }),
  });
  fetchPeople();
};
$("callBtn").onclick = () => {
  const box = $("callList");
  if (box.classList.contains("hidden")) openCallList(); else box.classList.add("hidden");
};
$("callList").onclick = (e) => {
  const ai = e.target.closest("[data-admit-invite]");
  if (ai) { admitAndInvite(ai.dataset.admitInvite); return; }
  const cancel = e.target.closest("[data-call-cancel]");
  if (cancel) { $("callList").classList.add("hidden"); return; }
  const row = e.target.closest("[data-call]");
  if (row) callIn(row.dataset.call);
};
$("lobbyList").onclick = (e) => {
  const row = e.target.closest("[data-lobby-call]");
  if (row) callIn(row.dataset.lobbyCall);
};
// Lobby is docked to the bottom, so increasing its height must happen upward.
// Pointer events cover mouse, pen and touch; remember the operator's chosen
// height on this browser.
(() => {
  const box = $("lobbyBox");
  const grip = $("lobbyResize");
  const saved = Number(localStorage.getItem("loca-lobby-height") || 66);
  const setHeight = (height, persist = false) => {
    const bounded = Math.max(48, Math.min(height, innerHeight * .55));
    box.style.height = `${bounded}px`;
    grip.setAttribute("aria-valuenow", String(Math.round(bounded)));
    if (persist) localStorage.setItem("loca-lobby-height", String(Math.round(bounded)));
  };
  grip.setAttribute("aria-valuemin", "48");
  grip.setAttribute("aria-valuemax", String(Math.round(innerHeight * .55)));
  setHeight(saved);
  let startY = 0, startHeight = 0;
  grip.onpointerdown = (e) => {
    startY = e.clientY;
    startHeight = box.getBoundingClientRect().height;
    grip.setPointerCapture(e.pointerId);
  };
  grip.onpointermove = (e) => {
    if (!grip.hasPointerCapture(e.pointerId)) return;
    const height = Math.max(48, Math.min(innerHeight * .55, startHeight + startY - e.clientY));
    setHeight(height);
  };
  grip.onpointerup = (e) => {
    if (grip.hasPointerCapture(e.pointerId)) grip.releasePointerCapture(e.pointerId);
    setHeight(box.getBoundingClientRect().height, true);
  };
  grip.ondblclick = () => {
    const height = box.getBoundingClientRect().height > 80 ? 66 : Math.min(260, innerHeight * .4);
    setHeight(height, true);
  };
  grip.onkeydown = (event) => {
    if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const current = box.getBoundingClientRect().height;
    if (event.key === 'ArrowUp') setHeight(current + 24, true);
    else if (event.key === 'ArrowDown') setHeight(current - 24, true);
    else if (event.key === 'Home') setHeight(48, true);
    else setHeight(innerHeight * .55, true);
  };
})();
$("tabTasks").onclick = () => switchTab("tasks");
$("tabJournal").onclick = () => switchTab("journal");
$("tabPeople").onclick = () => switchTab("people");
$("tkCreate").onclick = async () => {
  const title = $("tkTitle").value.trim();
  if (!title) return;
  if (await createTask(title, null, $("tkAssign").value.trim() || null)) {
    $("tkTitle").value = ""; $("tkAssign").value = "";
  }
};
$("saveReminders").onclick = applyReminderSettings;
function chooseReminderRecipient(kind) {
  $("reminderRecipientKind").value = kind;
  const person = kind === "person";
  $("reminderLeadChoice").classList.toggle("selected", !person);
  $("reminderPersonChoice").classList.toggle("selected", person);
  $("reminderAllChoice").classList.toggle("selected", kind === "all");
  $("reminderLeadChoice").classList.toggle("selected", kind === "lead");
  $("reminderLeadChoice").setAttribute("aria-checked", String(kind === "lead"));
  $("reminderPersonChoice").setAttribute("aria-checked", String(person));
  $("reminderAllChoice").setAttribute("aria-checked", String(kind === "all"));
  $("reminderPerson").classList.toggle("hidden", !person);
  $("reminderPerson").disabled = !person;
  const status = $("reminderSaveState");
  status.className = "remindersavestate";
  status.textContent = "Unsaved changes";
}
$("reminderLeadChoice").onclick = () => chooseReminderRecipient("lead");
$("reminderPersonChoice").onclick = () => chooseReminderRecipient("person");
$("reminderAllChoice").onclick = () => chooseReminderRecipient("all");
$("reminderPerson").onchange = () => {
  const status = $("reminderSaveState");
  status.className = "remindersavestate";
  status.textContent = "Unsaved changes";
};
for (const [checkId, inputId] of REMINDER_RULES) {
  $(checkId).onchange = () => {
    $(inputId).disabled = !$(checkId).checked;
    const status = $("reminderSaveState");
    status.className = "remindersavestate";
    status.textContent = "Unsaved changes";
  };
  $(inputId).oninput = () => {
    const status = $("reminderSaveState");
    status.className = "remindersavestate";
    status.textContent = "Unsaved changes";
  };
}
$("reminderHistoryList").addEventListener("click", async (e) => {
  const button = e.target.closest("[data-reminder-resolve]");
  if (!button) return;
  button.disabled = true;
  await attentionAct("resolve", button.dataset.reminderResolve);
});
$("goalCard").addEventListener("click", (e) => {
  const b = e.target.closest("[data-gact]");
  if (!b) return;
  if (b.dataset.gact === "edit") {
    const goal = state.goals[Number(b.dataset.gid)];
    if (!goal) return;
    switchTab("chat");
    $("msg").value = `@goal "${goal.outcome.replaceAll('"', '\\"')}"`;
    autoGrow();
    $("msg").focus();
    return;
  }
  goalAct(b.dataset.gact, Number(b.dataset.gid));
});
$("goalPanelCard").addEventListener("click", (e) => {
  if (!e.target.closest("[data-goal-prefill]")) return;
  const goal = currentGoal();
  switchTab("chat");
  $("msg").value = goal ? `@goal "${goal.outcome.replaceAll('"', '\\"')}"` : "@goal ";
  autoGrow();
  $("msg").focus();
});
$("waitList").addEventListener("click", (e) => {
  const b = e.target.closest("[data-clearwait]");
  if (b) clearWait(b.dataset.clearwait);
});
$("taskList").addEventListener("click", (e) => {
  const g = e.target.closest("[data-goto]");
  if (g) { e.preventDefault(); switchTab("chat"); gotoMsg(g.dataset.goto); return; }
  const b = e.target.closest("[data-tact]");
  if (b) taskAct(b.dataset.tact, Number(b.dataset.tid));
});
$("feed").addEventListener("click", (e) => {
  const picker = e.target.closest("[data-reactpick]");
  if (picker) {
    const panel = document.querySelector(`[data-picker="${picker.dataset.reactpick}"]`);
    document.querySelectorAll(".reactionpicker").forEach(p => { if (p !== panel) p.classList.add("hidden"); });
    panel?.classList.toggle("hidden");
    return;
  }
  const reaction = e.target.closest("[data-react][data-emoji]");
  if (reaction) {
    setReaction(Number(reaction.dataset.react), reaction.dataset.emoji).catch(() => addSys("reaction failed — connection error"));
    reaction.closest(".reactionpicker")?.classList.add("hidden");
    return;
  }
  const mk = e.target.closest("[data-mktask]");
  if (!mk) return;
  const m = msgById.get(Number(mk.dataset.mktask));
  if (!m) return;
  const title = prompt("Add a next step:", m.text.slice(0, 80));
  if (!title) return;
  const assignee = prompt("Assign to (may stay empty):", m.sender_type === "agent" ? m.sender : "");
  createTask(title.trim(), m.id, (assignee || "").trim() || null).then(ok => { if (ok) addSys(`Next step added: ${title.trim()}`); });
});
$("onlineList").addEventListener("click", (e) => {
  const b = e.target.closest("[data-mod]");
  if (b) moderate(b.dataset.mod, b.dataset.n);
  const lead = e.target.closest("[data-lead]");
  if (lead) setLead(lead.dataset.lead || null);
});
$("sendBtn").onclick = send;
$("leadSelect").addEventListener("change", async (e) => {
  const before = state.settings?.lead || "";
  const next = e.target.value || null;
  e.target.disabled = true;
  try {
    if (!(await setLead(next))) e.target.value = before;
  } catch (err) {
    e.target.value = before;
    addSys("could not change lead — connection failed");
  } finally {
    e.target.disabled = false;
  }
});
$("msg").addEventListener("keydown", (e) => {
  const popOpen = !$("mentionPop").classList.contains("hidden") && mentionMatches.length;
  if (popOpen && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    e.preventDefault();
    mentionSel = (mentionSel + (e.key === "ArrowDown" ? 1 : -1) + mentionMatches.length) % mentionMatches.length;
    [...$("mentionPop").children].forEach((c, i) => c.classList.toggle("sel", i === mentionSel));
    return;
  }
  if (popOpen && ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab")) { e.preventDefault(); pickMention(mentionMatches[mentionSel]); return; }
  if (popOpen && e.key === "Escape") { $("mentionPop").classList.add("hidden"); return; }
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); return; }
  // Shift+Enter is writing: let the newline through.
  signalTyping();
});
// grow with the text, up to the max-height set in CSS
function autoGrow() {
  const t = $("msg");
  t.style.height = "auto";
  t.style.height = Math.min(t.scrollHeight, window.innerHeight * 0.4) + "px";
}
$("msg").addEventListener("input", autoGrow);
$("msg").addEventListener("input", updateMentionPop);
$("mentionPop").addEventListener("click", (e) => {
  const d = e.target.closest("[data-i]");
  if (d) pickMention(mentionMatches[+d.dataset.i]);
});
$("replyCancel").onclick = clearReply;
$("jumpBtn").onclick = () => { scrollFeed(true); clearJump(); };
$("feed").addEventListener("scroll", () => { if (nearBottom()) clearJump(); });
$("newRoomBtn").onclick = createRoom;
$("newRoomName").addEventListener("keydown", (e) => { if (e.key === "Enter") createRoom(); });
$("feed").addEventListener("click", (e) => {
  const t = e.target.closest("[data-reply],[data-goto]");
  if (!t) return;
  if (t.dataset.reply) startReply(t.dataset.reply);
  else if (t.dataset.goto) gotoMsg(t.dataset.goto);
});
$("stopBtn").onclick = () => { if (state.ws && state.ws.readyState === 1) state.ws.send(JSON.stringify({ t: "control", cmd: "stop" })); };
$("clearBtn").onclick = () => { $("feed").innerHTML = ""; };

// tabs
$("tabChat").onclick = () => switchTab("chat");
$("tabNotes").onclick = () => switchTab("notes");

// notes create + edit (event delegation on the list)
$("nnCreate").onclick = createNote;
$("nnYou").textContent = state.name;
$("name").addEventListener("input", () => { $("nnYou").textContent = $("name").value.trim() || "operator"; });
$("noteList").addEventListener("click", (e) => {
  const t = e.target;
  if (t.dataset.edit) { state.editing = t.dataset.edit; renderNotes(); }
  else if (t.dataset.cancel) { state.editing = null; renderNotes(); }
  else if (t.dataset.save) { saveNote(t.dataset.save); }
  else if (t.dataset.del) { deleteNote(t.dataset.del); }
  else if (t.dataset.hist) { toggleHistory(t.dataset.hist); }
});

// The room remembers: past versions of a note, newest first.
async function toggleHistory(key) {
  const box = document.getElementById("nhist-" + key);
  if (!box) return;
  if (!box.classList.contains("hidden")) { box.classList.add("hidden"); return; }
  box.innerHTML = `<div class="hmeta">loading…</div>`;
  box.classList.remove("hidden");
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/notes/${encodeURIComponent(key)}/history`, { headers: adminHeaders({}) });
    const revs = await r.json();
    box.innerHTML = revs.length
      ? revs.map(v => `<div class="hrow"><div class="hmeta">rev ${v.rev} · ${esc(v.updated_by)} · ${fmtFull(v.updated_at)}</div><div class="hbody markdown">${renderMarkdown(v.body)}</div></div>`).join("")
      : `<div class="hmeta">no earlier versions</div>`;
  } catch (e) { box.innerHTML = `<div class="hmeta">history failed</div>`; }
}

async function deleteNote(key) {
  if (!confirm(`Delete note "${key}"?`)) return;
  await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/notes/${encodeURIComponent(key)}`, {
    method: "DELETE", headers: adminHeaders({}),
  });
  delete state.notes[key];
  renderNotes();
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    if (state.tab === "chat") markRoomRead(state.room, state.lastId);
    setTimeout(clearUnreadBoundary, 4000);
  }
});

// Start behind the door; /health tells us whether a key is even needed.
setLocked(true);
// Default server = where this page is served from. A host that wraps this UI
// (e.g. the desktop shell) can inject window.__LOCA_DEFAULT_SERVER__ at
// document-start to point the first connect at a different server; in a plain
// browser that global is undefined and this falls back to the page origin.
$("server").value = window.__LOCA_DEFAULT_SERVER__ || location.origin;
// Retake a remembered seat (auto-reload / next visit): no click needed.
try {
  const seat = JSON.parse(localStorage.getItem("loca-seat") || "null");
  if (seat && seat.name) {
    $("name").value = seat.name;
    $("roomToken").value = seat.roomToken || "";
    doConnect(seat.room || state.homeRoom);
  }
} catch (e) {}
refreshRooms();
