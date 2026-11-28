"use strict";
// Mentat rooms (docs: skill/agent-room/references/locas.md "Mentat room"):
// a loca founded as the place where one expert agent, the mentat, is
// consulted in turn. The server keeps the line and enforces every limit;
// this file shows it and sends the acts: ask, join or withdraw a request,
// end the consultation inside (saying whether it was answered), pause or
// resume the line, and, for the Master or the Smaster who founded the room,
// appoint the mentat. A rank watches from the door here and writes only in
// its own turn (state.js isWatchingDoor, admin.js canITalk).
let mentatView = null;   // {room, state, can_manage, is_mentat}
let mentatFetchSequence = 0;

function mentatRoomHere() { return !!state.settings?.mentat_room; }
function mentatName() { return state.settings?.mentat || ""; }

/// Is the consultation inside mine, and may I write in it now?
function mentatTurnIsMine() {
  const s = mentatView && mentatView.room === state.room ? mentatView.state : null;
  const current = s?.current;
  return !!current && !current.closing && !s.paused && (current.owners || []).includes(state.name);
}

async function fetchMentat(room) {
  const sequence = ++mentatFetchSequence;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/mentat`, { headers: adminHeaders({}) });
    if (room !== state.room || sequence !== mentatFetchSequence) return;
    mentatView = r.ok ? { room, ...(await r.json()) } : { room, state: null, can_manage: false };
  } catch (error) {
    if (room === state.room && sequence === mentatFetchSequence) mentatView = { room, state: null, can_manage: false };
  }
  renderMentatControl();
  renderMentatBar();
}

function onMentatFrame(f) {
  if (!f.state || f.state.mentat?.room !== state.room) return;
  mentatView = { ...(mentatView || {}), room: state.room, state: f.state };
  renderMentatBar();
}

// Called wherever the Lead control is drawn: the settings say whether this
// loca is a mentat room, and its line is fetched when that changes.
function renderMentatUI() {
  if (!mentatRoomHere() || !state.room) {
    mentatView = null;
    renderMentatControl();
    renderMentatBar();
    return;
  }
  if (!mentatView || mentatView.room !== state.room || (mentatView.state?.mentat?.mentat || "") !== mentatName()) {
    fetchMentat(state.room);
  } else {
    renderMentatControl();
    renderMentatBar();
  }
}

/// The Building's agents, for a mentat picker: online first. Those who are
/// already another room's mentat are listed but cannot be chosen.
async function loadMentatCandidates(select, current) {
  if (!select) return;
  const taken = new Map();
  try {
    const [residents, directory] = await Promise.all([
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/mentats`, { headers: adminHeaders({}) }),
    ]);
    const people = residents.ok ? await residents.json() : [];
    const rooms = directory.ok ? (await directory.json()).mentats || [] : [];
    for (const room of rooms) if (room.mentat && room.room !== state.room) taken.set(room.mentat, room.room);
    const agents = (Array.isArray(people) ? people : [])
      .filter(p => p.kind === "agent" && !p.suspended)
      .sort((a, b) => Number(!!b.online) - Number(!!a.online) || a.name.localeCompare(b.name));
    const keep = select.value;
    select.innerHTML = `<option value="">${current ? "No mentat (take the appointment back)" : "Choose later"}</option>` +
      agents.map(p => {
        const elsewhere = taken.get(p.name);
        const label = `${p.online ? "●" : "○"} ${p.name}${elsewhere ? ` — mentat of ${elsewhere}` : ""}`;
        return `<option value="${esc(p.name)}"${elsewhere ? " disabled" : ""}>${esc(label)}</option>`;
      }).join("");
    select.value = current || keep || "";
  } catch (error) {
    addSys("could not load the Building's agents");
  }
}

// The manager's picker in the header: who is this room's mentat.
function renderMentatControl() {
  const select = $("mentatSelect");
  if (!select) return;
  const show = mentatRoomHere() && !!state.room && !!mentatView?.can_manage;
  select.classList.toggle("hidden", !show);
  if (!show || select.dataset.room === state.room && select.dataset.mentat === mentatName()) return;
  select.dataset.room = state.room;
  select.dataset.mentat = mentatName();
  loadMentatCandidates(select, mentatName());
}

async function mentatAct(method, path, body) {
  const headers = adminHeaders(body ? { "content-type": "application/json" } : {});
  const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/mentat${path}`,
    { method, headers, body: body ? JSON.stringify(body) : undefined });
  if (!r.ok) addSys(`mentat: ${(await r.text()).slice(0, 200)}`);
  return r.ok;
}

function mentatClock(request, limits) {
  const now = Date.now();
  if (request.status === "called") {
    const left = Math.max(0, Math.round((request.called_at + limits.call_secs * 1000 - now) / 1000));
    return `called · ${left} s to start`;
  }
  if (request.closing) return "closing · the mentat's last answer";
  const left = Math.max(0, Math.round((request.started_at + limits.session_secs * 1000 - now) / 60000));
  return `${request.questions}/${limits.questions} questions · ${left} min left`;
}

function renderMentatBar() {
  const bar = $("mentatBar");
  if (!bar) return;
  const s = mentatView && mentatView.room === state.room ? mentatView.state : null;
  bar.classList.toggle("hidden", !s);
  if (typeof renderDoor === "function" && mentatRoomHere()) renderDoor();
  if (!s) { bar.replaceChildren(); return; }
  if (!s.mentat.mentat) {
    bar.innerHTML = `<span><b>This room has no mentat yet.</b> Nobody can ask until ` +
      `${mentatView.can_manage ? "you choose one in the header." : "the Master or the Smaster who founded it appoints one."}</span>`;
    return;
  }
  const me = state.name;
  const mine = (request) => (request.owners || []).includes(me);
  const holding = s.current && mine(s.current) ? s.current : s.waiting.find(mine);
  const isMentat = s.mentat.mentat === me || !!mentatView.is_mentat;
  const canRun = isMentat || !!mentatView.can_manage;
  const inside = s.current
    ? `Inside: ${s.current.owners.map(o => "@" + esc(o)).join(" ")} · ${esc(mentatClock(s.current, s.mentat.limits))}`
    : "Nobody inside";
  const waiting = s.waiting.length
    ? s.waiting.map(q => {
        const join = !holding && !isMentat ? ` <button type="button" data-mentat-join="${q.id}">join</button>` : "";
        const leave = mine(q) ? ` <button type="button" data-mentat-withdraw="${q.id}">withdraw</button>` : "";
        return `<span class="mentatq" title="${esc(q.topic)}">#${q.id} ${q.owners.map(o => "@" + esc(o)).join(" ")}${join}${leave}</span>`;
      }).join(" ")
    : "nobody waiting";
  const askerInside = s.current && mine(s.current);
  const ends = askerInside
    ? `<span class="mentatend">End: <button type="button" data-mentat-end="yes">answered</button>` +
      `<button type="button" data-mentat-end="partly">partly</button>` +
      `<button type="button" data-mentat-end="no">not answered</button></span>`
    : (s.current && canRun ? `<button type="button" data-mentat-end="">End consultation</button>` : "");
  const pause = canRun
    ? `<button type="button" id="mentatPause">${s.paused ? "Resume the line" : "Pause the line"}</button>`
    : "";
  bar.innerHTML = `<span><b>Mentat @${esc(s.mentat.mentat)}</b> · ${esc(s.mentat.topic)}${s.paused ? " · <b>paused</b>" : ""} · ${inside} · ${waiting}</span>
    <span class="bbacts">
      ${!holding && !isMentat ? `<button type="button" id="mentatAsk">Ask</button>` : ""}
      ${ends}${pause}
    </span>`;
}

$("mentatSelect")?.addEventListener("change", async (e) => {
  const before = mentatName();
  const next = e.target.value;
  e.target.disabled = true;
  try {
    const done = next
      ? await mentatAct("PUT", "", { mentat: next })
      : await mentatAct("DELETE", "");
    if (!done) e.target.value = before;
    else delete e.target.dataset.mentat;
  } catch (error) {
    e.target.value = before;
    addSys("could not change the mentat — connection failed");
  } finally {
    e.target.disabled = false;
  }
});

$("mentatBar")?.addEventListener("click", async (e) => {
  const join = e.target.closest("[data-mentat-join]");
  const withdraw = e.target.closest("[data-mentat-withdraw]");
  const end = e.target.closest("[data-mentat-end]");
  try {
    if (join) await mentatAct("POST", `/requests/${encodeURIComponent(join.dataset.mentatJoin)}/join`);
    else if (withdraw) await mentatAct("POST", `/requests/${encodeURIComponent(withdraw.dataset.mentatWithdraw)}/withdraw`);
    else if (end) await mentatAct("POST", "/end", end.dataset.mentatEnd ? { answered: end.dataset.mentatEnd } : {});
    else if (e.target.id === "mentatPause") {
      const paused = !!mentatView?.state?.paused;
      await mentatAct("POST", paused ? "/resume" : "/pause");
    } else if (e.target.id === "mentatAsk") {
      const topic = prompt("What do you want to ask the mentat about? One line, at least 10 characters. Everyone seated here reads it.");
      if (topic && topic.trim()) await mentatAct("POST", "/requests", { topic: topic.trim() });
    }
  } catch (error) {
    addSys("mentat: connection failed");
  }
});
