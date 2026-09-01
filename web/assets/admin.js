"use strict";
// Room mode, properties, reminder policy, and operator controls.
async function fetchMode() {
  if (!state.room) return;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/mode`, { headers: adminHeaders({}) });
    state.mode = await r.json();
  } catch (e) { state.mode = { mode: "free" }; }
  renderMode();
}

function renderMode() {
  const m = state.mode || { mode: "free" };
  const pill = $("modePill");
  pill.className = "pill " + m.mode;
  pill.textContent = m.mode === "roundrobin" ? "round-robin" : m.mode;
  let detail = "";
  if (m.mode === "free") detail = "anyone can talk";
  else if (m.mode === "paused") detail = "the table is quiet — the host holds the floor";
  else if (m.mode === "restricted") detail = `only: <b>${(m.allow || []).map(esc).join(", ") || "(nobody)"}</b>`;
  else if (m.mode === "roundrobin") {
    const who = (m.order || [])[m.turn];
    detail = `turn: <b>${esc(who || "?")}</b> &nbsp; (${(m.order || []).map(esc).join(" → ")})`;
  }
  $("modeDetail").innerHTML = detail;
  // gate the composer visually if this user can't talk right now
  $("composer").classList.toggle("gated", !canITalk());
  // reflect current mode in the admin selector
  if (isAdmin()) { $("modeSel").value = m.mode; }
  renderTopStatus();   // mode also shows in the header subtitle
}

function canITalk() {
  if (isAdmin()) return true;
  const m = state.mode || { mode: "free" };
  if (m.mode === "free") return true;
  if (m.mode === "paused") return false;
  if (m.mode === "restricted") return (m.allow || []).includes(state.name);
  if (m.mode === "roundrobin") return (m.order || [])[m.turn] === state.name;
  return true;
}

async function applyMode() {
  const sel = $("modeSel").value;
  const args = $("modeArg").value.split(",").map(s => s.trim()).filter(Boolean);
  let mode;
  if (sel === "free") mode = { mode: "free" };
  else if (sel === "paused") mode = { mode: "paused" };
  else if (sel === "restricted") mode = { mode: "restricted", allow: args };
  else if (sel === "roundrobin") mode = { mode: "roundrobin", order: args, turn: 0 };
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/mode`, {
    method: "PUT", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify({ mode }),
  });
  if (r.status === 401) { alert("admin token rejected"); return; }
  // live mode frame refreshes the banner.
}

/* ---- settings (rate limit) ---- */
let settingsFetchSequence = 0;
async function fetchSettings() {
  const room = state.room;
  if (!room) return;
  const sequence = ++settingsFetchSequence;
  let settings = {};
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/settings`, { headers: adminHeaders({}) });
    if (r.ok) settings = await r.json();
  } catch (e) {}
  if (sequence !== settingsFetchSequence || room !== state.room) return;
  state.settings = settings;
  markLocaContextReady("settings", room);
  renderSettings();
}
function renderSettings() {
  const s = state.settings || {};
  renderLeadControl();
  if (isAdmin()) {
    if (document.activeElement !== $("rlLimit")) $("rlLimit").value = s.rate_limit ?? 10;
    if (document.activeElement !== $("rlWindow")) $("rlWindow").value = s.rate_window_secs ?? 30;
    if (document.activeElement !== $("liveTimeout")) $("liveTimeout").value = s.live_timeout_secs ?? 120;
    if (document.activeElement !== $("opsList")) $("opsList").value = (s.operators || []).join(", ");
    if (document.activeElement !== $("turnMax")) $("turnMax").value = s.turn_max_messages ?? 4;
    if (document.activeElement !== $("turnIdle")) $("turnIdle").value = s.turn_idle_ms ?? 5000;
    if (document.activeElement !== $("turnHard")) $("turnHard").value = s.turn_max_wait_ms ?? 15000;
    if (document.activeElement !== $("careMax")) $("careMax").value = s.care_max_attempts ?? 2;
    if (document.activeElement !== $("careCtx")) $("careCtx").value = s.care_context_messages ?? 8;
    const lt = $("liveToggle");
    lt.classList.toggle("on", !!s.live);
    lt.textContent = s.live ? "🔴 Live: ON" : "◉ Live: off";
  }
  renderReminderSettings();
  renderLocaSidebar();
  // Live badge shows for EVERYONE (not just admin).
  $("liveBadge").classList.toggle("hidden", !s.live);
  // Archived rooms are read-only: say so and grey the composer.
  const arch = !!s.archived;
  $("archBadge").classList.toggle("hidden", !arch);
  $("closeLocaBtn").classList.toggle("hidden", arch);
  $("reopenLocaBtn").classList.toggle("hidden", !arch);
  $("sealLocaBtn").classList.toggle("hidden", !arch);
  $("composer").classList.toggle("gated", arch || !canITalk());
  $("msg").disabled = arch;
  $("msg").placeholder = arch ? "this loca is closed — read-only" : "say it to the table…  (Enter = send · Shift+Enter = new line)";
}
const REMINDER_RULES = [
  ["careGoalOn", "careGoal", "care_goal_secs", 10, "Goal"],
  ["careTaskOn", "careTask", "care_task_secs", 10, "Task"],
  ["careWaitOn", "careWait", "care_wait_secs", 2, "Declared wait"],
  ["careSilenceOn", "careSilence", "care_silence_secs", 30, "Room silence"],
];
const CARE_REASONS = new Set(["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"]);
function minutesValue(seconds, fallback) {
  if (!seconds) return fallback;
  return Math.round((seconds / 60) * 10) / 10;
}
function humanDuration(seconds) {
  if (seconds < 60) return `${seconds} sec`;
  if (seconds % 3600 === 0) return `${seconds / 3600} hr`;
  return `${Math.round((seconds / 60) * 10) / 10} min`;
}
function renderReminderSettings() {
  const s = state.settings || {};
  const configuredRecipient = s.care_recipient || { kind: "lead" };
  const recipientKind = configuredRecipient.kind === "person" ? "person"
    : configuredRecipient.kind === "all" ? "all" : "lead";
  const selectedPerson = recipientKind === "person" ? String(configuredRecipient.name || "") : "";
  $("reminderRecipientKind").value = recipientKind;
  $("reminderLeadChoice").textContent = state.settings?.lead ? `Lead @${state.settings.lead}` : "Room lead";
  $("reminderLeadChoice").classList.toggle("selected", recipientKind === "lead");
  $("reminderPersonChoice").classList.toggle("selected", recipientKind === "person");
  $("reminderAllChoice").classList.toggle("selected", recipientKind === "all");
  $("reminderLeadChoice").setAttribute("aria-checked", String(recipientKind === "lead"));
  $("reminderPersonChoice").setAttribute("aria-checked", String(recipientKind === "person"));
  $("reminderAllChoice").setAttribute("aria-checked", String(recipientKind === "all"));
  $("reminderLeadChoice").disabled = !isAdmin();
  $("reminderPersonChoice").disabled = !isAdmin();
  $("reminderAllChoice").disabled = !isAdmin();
  const knownNames = new Set([
    ...state.members.map(member => member.name),
    ...(state.seatedAway || []).map(member => member.name),
    ...(state.settings?.lead ? [state.settings.lead] : []),
    ...(selectedPerson ? [selectedPerson] : []),
  ]);
  $("reminderPerson").innerHTML = `<option value="">Choose a person…</option>` +
    [...knownNames].filter(Boolean).sort().map(name => `<option value="${esc(name)}">@${esc(name)}</option>`).join("");
  $("reminderPerson").value = selectedPerson;
  $("reminderPerson").classList.toggle("hidden", recipientKind !== "person");
  $("reminderPerson").disabled = !isAdmin() || recipientKind !== "person";
  for (const [checkId, inputId, key, fallback] of REMINDER_RULES) {
    const seconds = Number(s[key] || 0);
    const check = $(checkId);
    const input = $(inputId);
    check.checked = seconds > 0;
    check.disabled = !isAdmin();
    if (document.activeElement !== input) input.value = minutesValue(seconds, fallback);
    input.disabled = !isAdmin() || !check.checked;
  }
  if (document.activeElement !== $("careCool")) $("careCool").value = minutesValue(Number(s.care_cooldown_secs ?? 300), 5);
  $("careCool").disabled = !isAdmin();
  $("careMax").disabled = !isAdmin();
  $("careCtx").disabled = !isAdmin();
  $("saveReminders").classList.toggle("hidden", !isAdmin());

  const lead = s.lead;
  // "all" does NOT broadcast: it lets any healthy runtime own the follow-up
  // (vs a fixed lead/person). Say that, not "everyone", so the summary matches
  // the one-owner delivery (a care signal wakes a single owner, never the room).
  $("reminderRecipient").textContent = recipientKind === "all" ? "any healthy coordinator"
    : recipientKind === "person" && selectedPerson ? `@${selectedPerson}`
    : lead ? `@${lead}` : "@loca-care";
  $("reminderRecipientDetail").textContent = recipientKind === "all"
    ? "Visible to the whole room · one healthy runtime owns follow-up"
    : recipientKind === "person" && selectedPerson ? "Specific person · fallback @loca-care if unavailable"
    : lead
      ? "Room lead · fallback @loca-care if unavailable"
      : "No room lead is selected";
  const enabled = REMINDER_RULES.flatMap(([, , key, , label]) => {
    const seconds = Number(s[key] || 0);
    return seconds > 0 ? [`${label} after ${humanDuration(seconds)}`] : [];
  });
  const reminderOn = enabled.length > 0;
  const reminderUnavailable = reminderOn && recipientKind === "lead" && !lead;
  $("reminderStatus").classList.toggle("on", reminderOn && !reminderUnavailable);
  $("reminderMode").textContent = reminderUnavailable ? "Reminders unavailable"
    : reminderOn ? "Reminders active" : "Reminders off";
  $("reminderState").textContent = reminderUnavailable ? "NEEDS LEAD" : reminderOn ? "ON" : "OFF";
  $("reminderActiveSummary").textContent = enabled.length
    ? reminderUnavailable ? `Select a room lead or another recipient · ${enabled.join(" · ")}` : enabled.join(" · ")
    : "No signals selected.";
  $("reminderActiveSummary").title = enabled.join(" · ");

  const latest = Object.values(state.attentions)
    .filter(attention => CARE_REASONS.has(attention.reason))
    .sort((a, b) => b.created_at - a.created_at)[0];
  if (!latest) {
    $("reminderLastDelivery").textContent = "No reminder has fired yet. Saving a rule does not fire it immediately; its timer starts from real Goal, Task, Wait, or room activity.";
  } else {
    const recipient = latest.owner ? `@${latest.owner}` : "a coordinator";
    const delivery = latest.delivered_at ? "delivered" : "waiting for delivery";
    const lifecycle = latest.status === "resolved" ? "resolved" : latest.status === "claimed" ? "being handled" : "open";
    $("reminderLastDelivery").textContent = `Last reminder: ${latest.subject} → ${recipient} · ${delivery} · ${lifecycle}`;
  }
}
function reminderSettingsBody() {
  const body = {};
  const recipientKind = $("reminderRecipientKind").value;
  const recipientName = $("reminderPerson").value.trim();
  if (recipientKind === "person" && !recipientName) {
    throw new Error("Choose the person who should receive reminders.");
  }
  body.care_recipient = recipientKind === "person" ? { kind: "person", name: recipientName }
    : recipientKind === "all" ? { kind: "all" } : { kind: "lead" };
  for (const [checkId, inputId, key] of REMINDER_RULES) {
    if (!$(checkId).checked) { body[key] = 0; continue; }
    const minutes = parseFloat($(inputId).value);
    body[key] = Number.isFinite(minutes) ? Math.max(1, Math.round(minutes * 60)) : 0;
  }
  const cooldownMinutes = parseFloat($("careCool").value);
  const maxAttempts = parseInt($("careMax").value, 10);
  const contextMessages = parseInt($("careCtx").value, 10);
  if (Number.isFinite(cooldownMinutes)) body.care_cooldown_secs = Math.max(0, Math.round(cooldownMinutes * 60));
  if (!Number.isNaN(maxAttempts)) body.care_max_attempts = maxAttempts;
  if (!Number.isNaN(contextMessages)) body.care_context_messages = contextMessages;
  return body;
}
async function applySettings() {
  const rl = parseInt($("rlLimit").value, 10);
  const w = parseInt($("rlWindow").value, 10);
  const body = {};
  if (!Number.isNaN(rl)) body.rate_limit = rl;
  if (!Number.isNaN(w)) body.rate_window_secs = w;
  const lt = parseInt($("liveTimeout").value, 10);
  if (!Number.isNaN(lt)) body.live_timeout_secs = lt;
  const tm = parseInt($("turnMax").value, 10);
  const ti = parseInt($("turnIdle").value, 10);
  const th = parseInt($("turnHard").value, 10);
  if (!Number.isNaN(tm)) body.turn_max_messages = tm;
  if (!Number.isNaN(ti)) body.turn_idle_ms = ti;
  if (!Number.isNaN(th)) body.turn_max_wait_ms = th;
  try { Object.assign(body, reminderSettingsBody()); }
  catch (error) { alert(error.message); return; }
  body.operators = $("opsList").value.split(",").map(x => x.trim()).filter(Boolean);
  await putSettings(body);
}
async function applyReminderSettings() {
  const status = $("reminderSaveState");
  status.className = "remindersavestate";
  status.textContent = "Saving…";
  let body;
  try { body = reminderSettingsBody(); }
  catch (error) {
    status.classList.add("error"); status.textContent = error.message; return;
  }
  const hasActiveRule = REMINDER_RULES.some(([, , key]) => Number(body[key] || 0) > 0);
  const recoveredMissingLead = hasActiveRule
    && body.care_recipient?.kind === "lead"
    && !state.settings?.lead;
  if (recoveredMissingLead) {
    // A lead can be removed after a valid reminder was configured. Do not
    // leave the operator in an unsaveable form: use the explicit room-wide
    // recipient that is already part of the reminder contract.
    body.care_recipient = { kind: "all" };
    chooseReminderRecipient("all");
  }
  const result = await putSettings(body);
  status.classList.add(result.ok ? "saved" : "error");
  status.textContent = result.ok
    ? recoveredMissingLead
      ? "Saved for everyone — no room lead is set"
      : "Saved — active rules are shown above"
    : result.error;
}
async function putSettings(body) {
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/settings`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify(body),
    });
    const responseText = await r.text();
    if (r.status === 401) alert("admin token rejected");
    if (!r.ok) {
      return { ok: false, error: responseText.trim() || `Could not save (HTTP ${r.status})` };
    }
    state.settings = JSON.parse(responseText);
    renderSettings();
    return { ok: true, error: "" };
  } catch (error) {
    return { ok: false, error: "Could not reach the Loca server. Your changes were not saved." };
  }
}
async function toggleLive() {
  await putSettings({ live: !state.settings.live });   // live frame refreshes UI
}

/* ---- görevler (tasks): declaration, not a queue ---- */
function isLocaOperator() {
  return isAdmin() || (state.settings?.operators || []).includes(state.name);
}
/// Who holds a davet for this loca but is not connected right now. They still
/// occupy a seat, so the master must be able to see and release them.
let seatedFetchSequence = 0;
async function fetchSeated() {
  const room = state.room;
  const sequence = ++seatedFetchSequence;
  if (!room || !isAdmin()) {
    state.seatedAway = [];
    renderMembers();
    return;
  }
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/invites`, { headers: adminHeaders({}) });
    if (sequence !== seatedFetchSequence || room !== state.room) return;
    if (!r.ok) {
      state.seatedAway = [];
      renderMembers();
      return;
    }
    const invites = await r.json();
    if (sequence !== seatedFetchSequence || room !== state.room) return;
    const online = new Set(state.members.map(m => m.name));
    const seen = new Set();
    state.seatedAway = invites.filter(i => {
      if (online.has(i.name) || seen.has(i.name)) return false;
      seen.add(i.name);
      return true;
    });
    renderMembers();
  } catch (e) {
    if (sequence !== seatedFetchSequence || room !== state.room) return;
    state.seatedAway = [];
    renderMembers();
  }
}
