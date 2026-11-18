"use strict";
// Building lobby, room roster, presence, and people runtime health.
let residentRequestSequence = 0;
let residentAppliedSequence = 0;
function residentReadScope() {
  return { sequence: ++residentRequestSequence, server: serverBase(), session: state.session, name: state.name, gen: state.buildingGen };
}
function residentReadCurrent(scope) {
  return isAdmin() && scope.sequence >= residentAppliedSequence && scope.server === serverBase()
    && scope.session === state.session && scope.name === state.name && !buildingStale(scope.gen);
}
async function fetchLobby() {
  const box = $("lobbyBox");
  if (!isAdmin()) {
    state.lobby = [];
    box.classList.add("hidden");
    renderLobby();
    return;
  }
  box.classList.remove("hidden");
  const scope = residentReadScope();
  try {
    const r = await fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) });
    if (!r.ok) { if (residentReadCurrent(scope)) renderLobby("lobby unavailable"); return; }
    applyResidentSnapshot(await r.json(), { scope });
  } catch (e) {
    if (residentReadCurrent(scope)) renderLobby("lobby unavailable");
  }
}

// Keep every surface that renders Building presence on the same resident
// snapshot. The Building tab polls /residents while it is open; applying that
// response only to state.people left the sidebar Lobby stale until a reload or
// an unrelated websocket event arrived.
function applyResidentSnapshot(residents, { people = false, scope = null } = {}) {
  if (scope && !residentReadCurrent(scope)) return false;
  if (!Array.isArray(residents)) throw new Error("Invalid Building snapshot");
  if (scope) residentAppliedSequence = scope.sequence;
  if (people) state.people = residents;
  // Lobby is a live location state: connected to the Building, but seated
  // in no loca. Offline members remain visible in the Building directory as
  // disconnected; they must not inflate the Lobby count.
  state.lobby = residents.filter(p => !p.suspended && p.online && !(p.locas || []).length);
  renderLobby();
  return true;
}

function renderLobby(error) {
  const list = $("lobbyList");
  const rows = state.lobby || [];
  $("lobbyCount").textContent = rows.length;
  if (error) {
    list.innerHTML = `<div class="callnone">${esc(error)}</div>`;
    return;
  }
  if (!rows.length) {
    list.innerHTML = `<div class="callnone">empty</div>`;
    return;
  }
  list.innerHTML = rows.map(p => {
    const glyph = p.kind === "agent" ? "*" : ".";
    return `<div class="omem"><span class="glyph ${esc(p.kind)}">${glyph}</span>` +
      `<span class="oname">${esc(p.name)}</span><span class="otag">waiting</span>` +
      `${kitVersionHtml(p, "building")}${residentStatusBadges(p)}` +
      `<button class="lobbycall" data-lobby-call="${esc(p.name)}">call</button></div>`;
  }).join("");
}

function residentStatusBadges(p) {
  const connection = p?.online
    ? `<span class="pstatus connection ok" title="live lobby or loca socket">connected</span>`
    : `<span class="pstatus connection off" title="no live socket">disconnected</span>`;
  const wake = p?.kind !== "agent"
    ? `<span class="pstatus wake na" title="runtime wake does not apply to people">n/a</span>`
    : p?.runtime?.ready
    ? `<span class="pstatus wake ok" title="runtime wake + ACK healthy">ready</span>`
    : p?.runtime
      ? `<span class="pstatus wake bad" title="runtime heartbeat exists but wake is degraded">degraded</span>`
      : `<span class="pstatus wake unknown" title="no runtime heartbeat has been observed">unverified</span>`;
  return connection + wake;
}

/// Who is in the building but not in this loca — the people you can call in.
async function openCallList() {
  const box = $("callList");
  box.classList.remove("hidden");
  box.innerHTML = `<div class="callnone">looking…</div>`;
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) });
    if (buildingStale(gen)) return;
    if (!r.ok) { box.innerHTML = `<div class="callnone">could not fetch the list</div>`; return; }
    const all = await r.json();
    if (buildingStale(gen)) return;
    const here = new Set(state.members.map(m => m.name));
    // A group loca seats only its group's people: offer nobody else.
    const groupId = (state.rooms || []).find(rm => rm.room === state.room)?.group_id;
    let inGroup = null;
    if (groupId) {
      await fetchGroups(false);
      if (buildingStale(gen)) return;
      const group = (state.groups || []).find(g => g.id === groupId);
      inGroup = new Set((group?.members || []).map(m => m.name));
    }
    const free = all.filter(p => !p.suspended && !here.has(p.name) && !(p.locas || []).includes(state.room)
      && (!inGroup || inGroup.has(p.name)));
    if (!free.length) {
      box.innerHTML = `<div class="callnone">${inGroup ? "nobody else on this floor to call — add them to the floor first" : "nobody in the building to call"}</div>`;
      return;
    }
    box.innerHTML = free.map(p => {
      const where = (p.locas || []).length ? (p.locas || []).join(", ") : "lobby";
      const glyph = p.kind === "agent" ? "*" : ".";
      return `<button type="button" class="callrow" data-call="${esc(p.name)}">` +
             `<span>${glyph}${esc(p.name)}</span><span class="where">${esc(where)}</span></button>`;
    }).join("");
  } catch (e) {
    if (!buildingStale(gen)) box.innerHTML = `<div class="callnone">could not reach the server</div>`;
  }
}

/// Seat a member in this loca. They belong to the building already, so nothing
/// is handed over and nobody re-runs setup — they are simply in.
async function callIn(name) {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/call`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name }),
    });
    if (buildingStale(gen)) return;
    if (r.ok) {
      const v = await r.json().catch(() => ({}));
      if (buildingStale(gen)) return;
      if (v.already) {
        addSys(`${name} already has a seat in this loca`);
        offerReissue(name);
      } else {
        addSys(`${name} called into this loca`);
        $("callList").classList.add("hidden");
      }
      refreshRooms();
      fetchLobby();
      fetchSeated();
    } else if (r.status === 404) {
      // Not a building member. Admitting is its own act — offer it openly,
      // never do it silently behind an invite.
      offerAdmitAndInvite(name);
    } else {
      const why = await r.text();
      if (!buildingStale(gen)) addSys(`could not call ${name}: ${why}`);
    }
  } catch (e) { if (!buildingStale(gen)) addSys("call failed to send"); }
}

/// Someone who already holds a seat may have lost the invitation behind it
/// (an agent that was offline, a reinstalled machine). Re-sending replaces it
/// with a fresh one, delivered over their Lobby line like a call; the old one
/// stops working and the seat stays theirs.
function offerReissue(name) {
  const box = $("callList");
  box.classList.remove("hidden");
  box.innerHTML =
    `<div class="callnone">${esc(name)} already has a seat here. If their invitation was lost, send a fresh one; the current one stops working.</div>` +
    `<button type="button" class="callrow" data-call-reissue="${esc(name)}" data-room="${esc(state.room)}"><span>Re-send invitation</span></button>` +
    `<button type="button" class="callrow" data-call-cancel="1"><span>cancel</span></button>`;
}

async function reissueInvite(name, room) {
  const gen = state.buildingGen;
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/call`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name, reissue: true }),
    });
    if (buildingStale(gen)) return;
    if (!r.ok) {
      const why = await r.text();
      if (!buildingStale(gen)) addSys(`could not re-send ${name}'s invitation: ${why}`);
      return;
    }
    const v = await r.json().catch(() => ({}));
    if (buildingStale(gen)) return;
    $("callList").classList.add("hidden");
    // An older server ignores the flag and answers as to a call. Nothing was
    // re-sent then, and the person is told so.
    if (v.already) addSys(`this server cannot re-send invitations yet; ${name}'s invitation was not changed`);
    else addSys(`fresh invitation sent to ${name}; the previous one no longer works`);
    fetchSeated();
  } catch (e) { if (!buildingStale(gen)) addSys("re-send failed to send"); }
}

/// The open two-step for an outsider: admit to the building, then invite to
/// this loca. Two explicit requests, two records — no identity is created as
/// a side effect of an invite.
function offerAdmitAndInvite(name) {
  const box = $("callList");
  box.classList.remove("hidden");
  box.innerHTML =
    `<div class="callnone">${esc(name)} is not a building member.</div>` +
    `<button type="button" class="callrow" data-admit-invite="${esc(name)}"><span>admit &amp; invite</span></button>` +
    `<button type="button" class="callrow" data-call-cancel="1"><span>cancel</span></button>`;
}

async function admitAndInvite(name) {
  // Two steps of one person's decision: the second is not taken, into
  // whatever loca is open then, for whoever signed in after the first.
  const gen = state.buildingGen;
  try {
    const a = await fetch(`${serverBase()}/members`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name, kind: "agent" }),
    });
    if (buildingStale(gen)) return;
    if (!a.ok) { const why = await a.text(); if (!buildingStale(gen)) addSys(`could not admit ${name}: ${why}`); return; }
    addSys(`${name} admitted to the building`);
    await callIn(name);
  } catch (e) { if (!buildingStale(gen)) addSys("admit failed to send"); }
}

/// Building rank worn beside the name: the name says who, the badge says what.
function rankBadge(role) {
  if (role === "master") return ` <span class="rankbadge rank-master" title="Building Master">Master</span>`;
  if (role === "smaster") return ` <span class="rankbadge rank-smaster" title="Building Smaster">Smaster</span>`;
  return "";
}
/// The server refuses moderation that reaches up the Building ranks; do not
/// offer buttons it will refuse. Nobody moderates the Master; only the Master
/// moderates a Smaster.
function canModerateSeat(m) {
  if (m.role === "master") return false;
  if (m.role === "smaster") return state.profile?.building_role === "master";
  return true;
}
function renderMembers() {
  renderTopStatus();   // header subtitle shows online counts + mode
  renderLeadControl();
  if (typeof renderWikiEditor === "function") renderWikiEditor();
  renderLocaSidebar();

  // online panel in the sidebar
  $("callBtn").classList.toggle("hidden", !isAdmin() || !state.room);
  const ol = $("onlineList");
  ol.innerHTML = "";
  const muted = new Set(state.mod?.muted || []);
  // Everyone seated here, not only everyone currently connected. Somebody who
  // holds an invite but is offline still occupies a seat — and until now there
  // was no way to release them, because they were not on the list.
  const online = new Set(state.members.map(m => m.name));
  const seated = [
    ...state.members,
    ...(state.seatedAway || []).map(p => ({ name: p.name, type: p.kind, away: true })),
  ];
  $("onlineCount").textContent = seated.length;
  for (const m of seated) {
    const d = document.createElement("div");
    const isMe = m.name === state.name;
    const isLead = state.settings?.lead === m.name;
    d.className = "omem" + (isMe ? " self" : "") + (m.away ? " away" : "") +
      (isLead ? " is-lead" : "");
    if (isLead) d.title = `${m.name} is this loca's lead`;
    // The line itself, and a kick or ban waiting for a yes under it (roster.js).
    d.innerHTML = seatRowHtml(m, { isMe, isLead, muted: muted.has(m.name) });
    ol.appendChild(d);
    ol.insertAdjacentHTML("beforeend", seatAskHtml(m.name));
  }
  if (!seated.length) ol.innerHTML = `<div class="omem" style="color:var(--muted)">nobody seated</div>`;
  // Banned people aren't connected, so they'd be invisible — list them here
  // with an unban control (until now unban was API-only).
  const banned = state.mod?.banned || [];
  if (banned.length) {
    const bh = document.createElement("div");
    bh.className = "omem"; bh.style.color = "var(--muted)"; bh.style.fontSize = "11px";
    bh.textContent = "banned:";
    ol.appendChild(bh);
    for (const n of banned) {
      const d = document.createElement("div");
      d.className = "omem banned";
      const act = isAdmin() ? `<span class="omacts" style="opacity:1"><button data-mod="unban" data-n="${esc(n)}" title="unban">unban</button></span>` : "";
      d.innerHTML = `<span class="glyph" style="color:var(--bad)">×</span><span class="oname" style="color:var(--muted)">${esc(n)}</span><span class="otag" style="color:var(--bad)">banned</span>${act}`;
      ol.appendChild(d);
    }
  }
  syncSeatMenu();
  const sel = $("target");
  const cur = sel.value;
  sel.innerHTML = `<option value="">— (wall)</option><option value="all">@all</option>`;
  for (const m of state.members) {
    if (m.name === state.name) continue;
    const o = document.createElement("option");
    o.value = m.name; o.textContent = "@" + m.name; sel.appendChild(o);
  }
  if ([...sel.options].some(o => o.value === cur)) sel.value = cur;
  if (typeof refreshSenderCards === "function") refreshSenderCards();
}

function avatarColor(name) {
  let h = 0; for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 60% 62%)`;
}
// WhatsApp-style short time (HH:MM); full date on hover.
// A clock time in the reader's chosen style (Profile → Reading): the
// device's own, 24-hour or 12-hour. "How long ago" applies to message lines
// only (fmtMessageTime); elsewhere a time stays a clock time.
function timeStyle() {
  return (state.appearance || cachedAppearance()).time_format;
}
function fmtMessageTime(ts, now = Date.now()) {
  return timeStyle() === "relative" ? relativeTime(ts, now) : fmtTime(ts);
}
function fmtTime(ts) {
  const style = timeStyle();
  const options = { hour: "2-digit", minute: "2-digit" };
  if (style === "h24") options.hourCycle = "h23";
  if (style === "h12") { options.hour12 = true; options.hour = "numeric"; }
  return new Date(ts).toLocaleTimeString([], options);
}
function relativeTime(ts, now = Date.now()) {
  const seconds = Math.round((now - Number(ts)) / 1000);
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(Number(ts)).toLocaleDateString([], { month: "short", day: "numeric" });
}
// Redraw the times already in the feed: after the style changes, and each
// minute while "how long ago" is chosen.
function refreshFeedTimes() {
  for (const row of document.querySelectorAll("#feed .row[data-ts]")) {
    const ts = Number(row.dataset.ts || 0);
    const time = row.querySelector(".line .time");
    if (ts && time) time.textContent = fmtMessageTime(ts);
  }
}
setInterval(() => {
  if (state.appearance?.time_format === "relative") refreshFeedTimes();
}, 60_000);
function fmtFull(ts) {
  return ts ? new Date(ts).toLocaleString() : "";
}
const msgById = new Map();  // id -> message (for reply quotes)
let keepScroll = null;      // reader position preserved across a resync
let jumpCount = 0;          // unseen messages behind the ↓ button
function bumpJump() {
  jumpCount++;
  const b = $("jumpBtn");
  b.textContent = `↓ ${jumpCount} new`;
  b.classList.remove("hidden");
}
function clearJump() { jumpCount = 0; $("jumpBtn").classList.add("hidden"); }
let lastDayKey = null;      // day separator state (reset when feed clears)

function dayKey(ts) { const d = new Date(ts); return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`; }
function dayLabel(ts) {
  const d = new Date(ts), now = new Date();
  const today = dayKey(now.getTime());
  const yest = dayKey(now.getTime() - 864e5);
  const k = dayKey(ts);
  if (k === today) return "Today";
  if (k === yest) return "Yesterday";
  return d.toLocaleDateString([], { day: "numeric", month: "long", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
}
function maybeDayChip(ts) {
  if (!ts) return;
  const k = dayKey(ts);
  if (k === lastDayKey) return;
  lastDayKey = k;
  const c = document.createElement("div");
  c.className = "daychip";
  c.textContent = dayLabel(ts);
  $("feed").appendChild(c);
}

async function fetchPeople() {
  // Building management also works before taking a loca seat. Hydrate the
  // verified Building role rather than relying on a room socket to fetch it.
  if (isAdmin()) await fetchProfile();
  if (!isAdmin()) return;
  const scope = residentReadScope();
  const box = $("peopleList");
  try {
    const [resBans, resPeople, resJoin, resStock, resRanks] = await Promise.all([
      fetch(`${serverBase()}/building/bans`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/join-requests`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/admission-stock`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/building/ranks`, { headers: adminHeaders({}) }),
    ]);
    if (!resPeople.ok) { if (residentReadCurrent(scope)) box.innerHTML = `<div class="callnone">could not load the list</div>`; return; }
    const people = await resPeople.json();
    if (!residentReadCurrent(scope)) return;
    // Each body below is one more await: a sign-out may land between any two.
    // Durable source of truth for admissions: reloaded from the server on every
    // open/reconnect, so a Master always sees pending requests even if the live
    // "wants to join" chat nudge was missed. On a failed fetch we KEEP the last
    // known list and flag the error — a fetch failure must never masquerade as
    // "no pending requests".
    if (resJoin.ok) {
      const pending = (await resJoin.json()).pending || [];
      if (buildingStale(scope.gen)) return;
      state.joinRequests = pending;
      state.joinRequestsError = false;
    } else {
      state.joinRequestsError = true;
    }
    if (resStock.ok) { const stock = await resStock.json(); if (buildingStale(scope.gen)) return; state.admissionStock = stock; }
    if (resRanks.ok) { const ranks = await resRanks.json(); if (buildingStale(scope.gen)) return; state.ranks = ranks; }

    // A ban lives in the room, not on the person: one request returns every
    // loca's bans that this viewer may see, regrouped here by person.
    const bans = {};
    const banRows = resBans.ok ? await resBans.json().catch(() => []) : [];
    if (buildingStale(scope.gen)) return;
    for (const row of Array.isArray(banRows) ? banRows : []) {
      if (isDm(row.room)) continue;
      (row.banned || []).forEach(n => { (bans[n] ||= []).push(row.room); });
    }

    // Groups give the People tab its group column and filter.
    await fetchGroups(false);
    if (buildingStale(scope.gen)) return;
    if (!applyResidentSnapshot(people, { people: true, scope })) return;
    state.bans = bans;
    renderPeople();
    fetchDmOverview();
    fetchKeyLedger();
  } catch (e) {
    if (residentReadCurrent(scope)) box.innerHTML = `<div class="callnone">could not reach the server</div>`;
  }
}

// Building-level admissions surface: pending join requests (the durable list,
// reloaded from the server), plus admission stock, all in the MAIN app so a
// Master approves here rather than in the hidden SSH-tunnel desk. Empty, it
// collapses to one quiet line; it only becomes a card when someone waits.
function joinRequestsHtml() {
  const loaded = Array.isArray(state.joinRequests);
  // Not fetched yet (and no error): render nothing rather than a misleading box.
  if (!loaded && !state.joinRequestsError) return "";
  const jr = loaded ? state.joinRequests : [];
  const stock = state.admissionStock;
  const gauge = stock
    ? `${stock.available} of ${stock.total} admissions left`
    : "admissions —";
  let rows;
  if (state.joinRequestsError && !jr.length) {
    // A failed fetch is shown AS an error — never as "no pending requests".
    rows = `<div class="jrnone jrerr">could not load join requests — retrying…</div>`;
  } else if (!jr.length) {
    rows = `<div class="jrnone">no pending join requests</div>`;
  } else {
    // A stale list after a failed refresh STILL carries an explicit error banner,
    // so the Master never mistakes last-known rows for a current, complete list.
    const stale = state.joinRequestsError
      ? `<div class="jrerr jrstale">could not refresh — showing last known list</div>`
      : "";
    rows = stale + jr.map(r => {
      const kind = r.kind === "user" ? "human" : r.kind === "agent" ? "agent" : "unknown";
      return `<div class="jrrow"><span class="jrwho">${kindMark(kind)}<b>${esc(r.name)}</b>` +
        `<span class="pkind">${esc(kind === "unknown" ? "Unknown type" : kind)}</span> wants to join</span>` +
        `<span class="jracts"><button class="btn primary" data-jr-approve="${esc(r.id)}">Approve</button>` +
        `<button class="btn" data-jr-deny="${esc(r.id)}">Deny</button></span></div>`;
    }).join("");
  }
  const waiting = jr.length ? " waiting" : "";
  return `<section class="jrsection${waiting}">` +
    `<div class="jrhead"><span class="bsection">Join requests${jr.length ? ` <span class="bcount">${jr.length}</span>` : ""}</span>` +
    `<span class="jrstock">${esc(gauge)}<button class="btn" data-jr-mint="5" title="add five admission rights">Mint 5</button></span></div>` +
    rows +
    `<div class="jrnotice" id="jrNotice"></div></section>`;
}

// A small coloured mark says human or agent; the word sits beside it.
function kindMark(kind) {
  return `<span class="kindmark ${kind === "agent" ? "agent" : kind === "human" || kind === "user" ? "human" : ""}" aria-hidden="true"></span>`;
}

// Where someone is, as loca chips; nothing is a quiet dash-free sentence.
function whereHtml(locas, fallback) {
  if (locas && locas.length) {
    // A busy agent sits in many locas: show three, then "+N" with the rest
    // in its tooltip, so the column never spills into the next one.
    // A loca shows its title; the id stays in the tooltip.
    const shown = locas.slice(0, 3).map(l =>
      `<span class="locachip" title="seated in ${esc(locaName(l))}${locaTitle(l) ? ` (id: ${esc(l)})` : ""}">#${esc(locaName(l))}</span>`).join("");
    const rest = locas.slice(3);
    return shown + (rest.length
      ? `<span class="locachip more" title="${esc(rest.map(l => "#" + locaName(l)).join("\n"))}">+${rest.length}</span>` : "");
  }
  return `<span class="pnowhere">${esc(fallback)}</span>`;
}

function renderPeople() {
  const box = $("peopleList");
  const people = state.people || [];
  const bans = state.bans || {};
  const names = new Set(people.map(p => p.name));
  Object.keys(bans).forEach(n => names.add(n));   // barred people may hold no seat
  const ranks = Array.isArray(state.ranks) ? state.ranks : [];
  renderBuildingSummary();

  const jrHtml = joinRequestsHtml();
  // Ranks hold no Lobby membership, so the resident directory never lists
  // them. They are still part of the Building: show them first, by name.
  const ranksHtml = ranks.length
    ? `<section class="bblock"><div class="bsection">Ranks <span class="bcount">${ranks.length}</span></div>` +
      `<div class="peoplehead"><span>name</span><span>seat</span><span>floors</span><span>location</span><span></span></div>` +
      ranks.filter(r => r.role === "master" || personInGroupFilter(r.name)).map(r => {
        const seated = (r.locas || []).length;
        // Location already says where; the status only marks a live seat.
        // An issued but never-used Smaster code is a code, not a person.
        const status = r.unused
          ? `<span class="pstatus warn" title="This Smaster code was issued but nobody has signed in with it">unused code</span>`
          : seated
            ? `<span class="pstatus connection ok" title="seated in a loca right now">seated</span>`
            : "";
        const where = r.unused
          ? `<span class="pnowhere">nobody has signed in with it yet</span>`
          : whereHtml(r.locas, "not in a loca");
        // A Smaster names themselves at sign-in; the Master only removes,
        // after a yes in words under the row (keys.js removeSmaster).
        const manage = state.profile?.building_role === "master" && r.role === "smaster";
        const remove = manage
          ? `<button type="button" class="btn danger" data-remove-smaster="${esc(r.name)}" title="Remove this Smaster from the Building">Remove</button>`
          : "";
        const ask = manage && state.removingSmaster?.name === r.name && state.removingSmaster.gen === state.buildingGen
          ? removeSmasterAskHtml(r) : "";
        const groups = r.role === "master"
          ? `<span class="pnowhere">sees every floor</span>` : groupChipsHtml(r.name);
        return `<div class="prow rankrow${r.unused ? " unusedcode" : ""}"><span class="pname">${kindMark("human")}${personNameHtml(r.name, cardOf(r))}${rankBadge(r.role)}</span>` +
          `<span class="pstatuses">${status}</span><span class="pgroups">${groups}</span><span class="pwhere">${where}</span><span class="pacts">${remove}</span></div>` + ask;
      }).join("") + `</section>`
    : "";
  if (!names.size) {
    box.innerHTML = jrHtml + groupFilterHtml() + ranksHtml +
      `<section class="bblock"><div class="bsection">Members</div><div class="callnone">no members in the building yet</div></section>`;
    return;
  }
  const isMaster = state.profile?.building_role === "master";
  const shown = [...names].filter(personInGroupFilter);
  const membersHtml = shown.sort((a, b) => a.localeCompare(b)).map(name => {
    const p = people.find(x => x.name === name);
    const kind = p?.kind === "agent" ? "agent" : "human";
    const locas = p?.locas || [];
    const barred = bans[name] || [];
    const rt = p?.runtime;
    let stage = "";
    if (rt?.attention_id) {
      const shortId = String(rt.attention_id).split(":").slice(-1)[0];
      if (rt.final_response) stage = `<span class="pstage final" title="final response accepted by Loca · ${esc(shortId)}">responded</span>`;
      else if (rt.turn_completed) stage = `<span class="pstage relay" title="Agent turn completed; final relay still pending · ${esc(shortId)}">relay pending</span>`;
      else if (rt.first_response) stage = `<span class="pstage first" title="first response accepted; work continues · ${esc(shortId)}">replied</span>`;
      else if (rt.accepted) stage = `<span class="pstage accepted" title="runtime accepted this attention · ${esc(shortId)}">accepted</span>`;
      else if (rt.stored) stage = `<span class="pstage" title="attention stored durably · ${esc(shortId)}">queued</span>`;
    }
    const fallback = p?.online ? "lobby" : "offline";
    const suspended = p?.suspended ? `<span class="pflag warn">Suspended</span>` : "";
    const banTxt = barred.length
      ? `<span class="pflag bad" title="banned from ${esc(barred.join(", "))}">banned: ${esc(barred.join(", "))}</span>` : "";
    const unban = barred.map(r =>
      `<button class="btn" data-unban="${esc(name)}" data-room="${esc(r)}">Unban #${esc(r)}</button>`).join("");
    const manage = isMaster && p?.membership_id
      ? `<button type="button" class="btn" data-suspend-member="${esc(p.membership_id)}" data-suspended="${p.suspended ? "0" : "1"}" data-member-name="${esc(name)}">${p.suspended ? "Resume" : "Suspend"}</button>` +
        `<button type="button" class="btn danger" title="Revoke membership" data-revoke-member="${esc(p.membership_id)}" data-member-name="${esc(name)}">Revoke</button>` : "";
    return `<div class="prow${p?.suspended ? " suspended" : ""}">` +
      `<span class="pname">${kindMark(kind)}${personNameHtml(name, cardOf(p))}<span class="pkind">${kind}</span>${kitVersionHtml(p, "building", "pkind")}${stage}</span>` +
      `<span class="pstatuses">${residentStatusBadges(p)}</span>` +
      `<span class="pgroups">${groupChipsHtml(name)}</span>` +
      `<span class="pwhere">${suspended}${whereHtml(locas, fallback)}${banTxt}</span>` +
      `<span class="pacts">${name !== state.name ? `<button type="button" class="btn" data-dm="${esc(name)}" title="Write to ${esc(name)} privately">Message</button>` : ""}${unban}${manage}</span></div>`;
  }).join("");
  box.innerHTML = jrHtml + groupFilterHtml() + ranksHtml +
    `<section class="bblock"><div class="bsection">Members <span class="bcount">${shown.length}</span></div>` +
    `<div class="peoplehead"><span>name</span><span>connection · wake</span><span>floors</span><span>location</span><span></span></div>` +
    (membersHtml || `<div class="callnone">nobody on this floor</div>`) + `</section>`;
}

// A name opens that person's card (who they are, and their groups). `card`
// comes from the row itself, which the server resolved to its principal.
function personNameHtml(name, card) {
  return `<button type="button" class="pnametext pnamebtn" data-person-card="${esc(name)}" title="Open ${esc(name)}'s card">${personLabelHtml(name, card || null)}</button>`;
}

// ---- Full name and profile icon ------------------------------------------
// Display only (docs/concepts.md "Full name and profile icon"): the @name is
// who someone is, so wherever a full name is shown the @name stands beside
// it. The icons, avatar colours and shapes are closed lists the server
// validates; they are drawn in avatar.js.

// The card a person-describing object carries, or null.
function cardOf(person) {
  if (!person) return null;
  const fullName = typeof person.full_name === "string" ? person.full_name : "";
  const icon = person.icon === "initials" || Object.hasOwn(PROFILE_ICONS, person.icon) ? person.icon : "";
  const color = Object.hasOwn(AVATAR_COLORS, person.avatar_color) ? person.avatar_color : "";
  const shape = AVATAR_SHAPES.includes(person.avatar_shape) ? person.avatar_shape : "";
  // The @name the object names, for initials only (never to look a card up).
  const name = typeof (person.display_name ?? person.name) === "string" ? (person.display_name ?? person.name) : "";
  return fullName || icon || color
    ? { full_name: fullName, icon, avatar_color: color, avatar_shape: shape, name } : null;
}

// The card of one principal, by id: what a message's writer wears. Only a
// message the server recorded a verified writer for has an id; anything
// else shows its plain name. A revoked person is in none of these lists.
function personCardById(id) {
  if (!id) return null;
  const seated = (state.members || []).find(m => m.principal_id === id);
  if (seated) return cardOf(seated);
  if (state.profile?.principal?.id === id) return cardOf(state.profile.principal);
  // The other side of a private conversation, seated here or not.
  const peer = (state.dms || []).find(b => b.peer_principal === id);
  if (peer) return dmPeerCard(peer);
  const lists = [
    [state.people, "principal_id"], [state.ranks, "principal_id"], [state.profilesList, "id"],
  ];
  for (const [list, key] of lists) {
    const found = Array.isArray(list) ? list.find(p => p[key] === id) : null;
    if (found) return cardOf(found);
  }
  return null;
}

// What this client knows about an @name, for what lists people (mention
// completion, a person's card, the key ledger), never for words someone
// wrote. The roster is authoritative for the people seated here: a seat the
// server did not verify carries no card, and that is not overridden by
// another list.
function personCardFor(name) {
  if (!name) return null;
  const seated = (state.members || []).find(m => m.name === name);
  if (seated) return cardOf(seated);
  if (state.profile?.principal?.display_name === name) return cardOf(state.profile.principal);
  const lists = [
    [state.people, "name"], [state.ranks, "name"], [state.profilesList, "display_name"],
  ];
  for (const [list, key] of lists) {
    const found = Array.isArray(list) ? list.find(p => p[key] === name) : null;
    if (found) return cardOf(found);
  }
  return null;
}

// "[icon] Full Name @name" when a full name is set, "[icon] name" otherwise,
// exactly "name" with neither. Escaped; the full name is isolated (<bdi>) so
// right-to-left text cannot move what stands around it.
function personLabelHtml(name, card) {
  const icon = avatarHtml(card, name, "xs");
  if (!card?.full_name) return `${icon}${esc(name)}`;
  return `${icon}<bdi class="fullname">${esc(card.full_name)}</bdi> <span class="atname">@${esc(name)}</span>`;
}

// The Building header counts what the directory actually shows.
function renderBuildingSummary() {
  if (state.tab !== "people") return;
  const people = state.people || [];
  const ranks = Array.isArray(state.ranks) ? state.ranks.length : 0;
  const online = people.filter(p => p.online).length;
  const lobby = people.filter(p => p.online && !(p.locas || []).length).length;
  const parts = [];
  if (ranks) parts.push(`${ranks} rank${ranks === 1 ? "" : "s"}`);
  parts.push(`${people.length} member${people.length === 1 ? "" : "s"}`);
  parts.push(`${online} online`);
  parts.push(`${lobby} in lobby`);
  $("subStatus").textContent = parts.join(" · ");
}

async function refreshPeopleRuntime() {
  if (!isAdmin() || state.tab !== "people") return;
  const scope = residentReadScope();
  try {
    const [resPeople, resJoin, resStock, resRanks] = await Promise.all([
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/join-requests`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/admission-stock`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/building/ranks`, { headers: adminHeaders({}) }),
    ]);
    if (!resPeople.ok) return;
    if (!applyResidentSnapshot(await resPeople.json(), { people: true, scope })) return;
    // Keep the admissions panel live while the tab is open: a request that
    // arrives (or is approved elsewhere) shows up within the poll interval.
    if (resJoin.ok) {
      const pending = (await resJoin.json()).pending || [];
      if (buildingStale(scope.gen)) return;
      state.joinRequests = pending;
      state.joinRequestsError = false;
    } else {
      state.joinRequestsError = true; // keep the last known list; flag the error
    }
    if (resStock.ok) { const stock = await resStock.json(); if (buildingStale(scope.gen)) return; state.admissionStock = stock; }
    if (resRanks.ok) { const ranks = await resRanks.json(); if (buildingStale(scope.gen)) return; state.ranks = ranks; }
    renderPeople();
  } catch (e) { /* the normal health indicator owns transport errors */ }
}
