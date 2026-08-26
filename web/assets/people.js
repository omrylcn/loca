"use strict";
// Building lobby, room roster, presence, and people runtime health.
async function fetchLobby() {
  const box = $("lobbyBox");
  if (!isAdmin()) {
    state.lobby = [];
    box.classList.add("hidden");
    renderLobby();
    return;
  }
  box.classList.remove("hidden");
  try {
    const r = await fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) });
    if (!r.ok) { state.lobby = []; renderLobby("lobby unavailable"); return; }
    const residents = await r.json();
    state.lobby = residents.filter(p => !(p.locas || []).length);
    renderLobby();
  } catch (e) {
    state.lobby = [];
    renderLobby("lobby unavailable");
  }
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
      `<button class="lobbycall" data-lobby-call="${esc(p.name)}">call</button></div>`;
  }).join("");
}

/// Who is in the building but not in this loca — the people you can call in.
async function openCallList() {
  const box = $("callList");
  box.classList.remove("hidden");
  box.innerHTML = `<div class="callnone">looking…</div>`;
  try {
    const r = await fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) });
    if (!r.ok) { box.innerHTML = `<div class="callnone">could not fetch the list</div>`; return; }
    const all = await r.json();
    const here = new Set(state.members.map(m => m.name));
    const free = all.filter(p => !here.has(p.name) && !(p.locas || []).includes(state.room));
    if (!free.length) {
      box.innerHTML = `<div class="callnone">nobody in the building to call</div>`;
      return;
    }
    box.innerHTML = free.map(p => {
      const where = (p.locas || []).length ? (p.locas || []).join(", ") : "lobby";
      const glyph = p.kind === "agent" ? "*" : ".";
      return `<button type="button" class="callrow" data-call="${esc(p.name)}">` +
             `<span>${glyph}${esc(p.name)}</span><span class="where">${esc(where)}</span></button>`;
    }).join("");
  } catch (e) {
    box.innerHTML = `<div class="callnone">could not reach the server</div>`;
  }
}

/// Seat a member in this loca. They belong to the building already, so nothing
/// is handed over and nobody re-runs setup — they are simply in.
async function callIn(name) {
  try {
    const r = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/call`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name }),
    });
    if (r.ok) {
      const v = await r.json().catch(() => ({}));
      if (v.already) addSys(`${name} already has a seat in this loca`);
      else addSys(`${name} called into this loca`);
      $("callList").classList.add("hidden");
      refreshRooms();
      fetchLobby();
      fetchSeated();
    } else if (r.status === 404) {
      // Not a building member. Admitting is its own act — offer it openly,
      // never do it silently behind an invite.
      offerAdmitAndInvite(name);
    } else {
      addSys(`could not call ${name}: ${await r.text()}`);
    }
  } catch (e) { addSys("call failed to send"); }
}

/// The open two-step for an outsider: admit to the building, then invite to
/// this loca. Two explicit requests, two records — no identity is created as
/// a side effect of a davet.
function offerAdmitAndInvite(name) {
  const box = $("callList");
  box.classList.remove("hidden");
  box.innerHTML =
    `<div class="callnone">${esc(name)} is not a building member.</div>` +
    `<button type="button" class="callrow" data-admit-invite="${esc(name)}"><span>admit &amp; invite</span></button>` +
    `<button type="button" class="callrow" data-call-cancel="1"><span>cancel</span></button>`;
}

async function admitAndInvite(name) {
  try {
    const a = await fetch(`${serverBase()}/members`, {
      method: "POST",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ name, kind: "agent" }),
    });
    if (!a.ok) { addSys(`could not admit ${name}: ${await a.text()}`); return; }
    addSys(`${name} admitted to the building`);
    await callIn(name);
  } catch (e) { addSys("admit failed to send"); }
}

function renderMembers() {
  renderTopStatus();   // header subtitle shows online counts + mode
  renderLeadControl();
  renderLocaSidebar();

  // online panel in the sidebar
  $("callBtn").classList.toggle("hidden", !isAdmin() || !state.room);
  const ol = $("onlineList");
  ol.innerHTML = "";
  const muted = new Set(state.mod?.muted || []);
  // Everyone seated here, not only everyone currently connected. Somebody who
  // holds a davet but is offline still occupies a seat — and until now there
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
    const flag = muted.has(m.name) ? ` <span class="mflag">🔇</span>` : "";
    // The title is worn wherever the name appears: it was named out loud, so it
    // should be visible at a glance rather than buried in settings.
    const leadTag = isLead ? ` <span class="leadtag" title="loca lead">lead</span>` : "";
    // The lead title must be visible in the sidebar before the operator has to
    // open settings or read a badge: the seat itself wears a blue diamond.
    const glyph = isLead ? "◆" : (m.type === "agent" ? "*" : ".");
    const glyphClass = `${m.type}${isLead ? " lead" : ""}`;
    // Admin gets the explicit lead action; chat text never changes authority.
    const leadAct = isAdmin()
      ? (isLead
        ? `<button data-lead="" title="end lead">◇</button>`
        : `<button data-lead="${esc(m.name)}" title="name as lead">◆</button>`)
      : "";
    // Admin (not on self) gets moderation buttons.
    let acts = "";
    if (isAdmin()) {
      let moderation = "";
      if (!isMe) {
        const mu = muted.has(m.name)
          ? `<button data-mod="unmute" data-n="${esc(m.name)}" title="unmute">🔊</button>`
          : `<button data-mod="mute" data-n="${esc(m.name)}" title="mute">🔇</button>`;
        // Release first: finishing a job is the common case, and it must not
        // look like a punishment sitting next to kick and ban.
        moderation =
          `<button class="release" data-mod="release" data-n="${esc(m.name)}" title="işi bitti — locadan çıkar, bina üyeliği kalır">release</button>` +
          `${mu}<button data-mod="kick" data-n="${esc(m.name)}" title="kick — close the connection and stop the davet">👢</button>` +
          `<button data-mod="ban" data-n="${esc(m.name)}" title="ban — bu locaya bir daha giremez">🚫</button>`;
      }
      acts = `<span class="omacts">${leadAct}${moderation}</span>`;
    }
    d.innerHTML = `<span class="glyph ${glyphClass}" title="${isLead ? "loca lead" : m.type}">${glyph}</span><span class="oname">${esc(m.name)}${isMe ? " (you)" : ""}${flag}${leadTag}</span><span class="otag">${m.type}</span>${acts}`;
    ol.appendChild(d);
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
      const act = isAdmin() ? `<span class="omacts" style="opacity:1"><button data-mod="unban" data-n="${esc(n)}" title="unban">🔓</button></span>` : "";
      d.innerHTML = `<span class="glyph" style="color:var(--bad)">×</span><span class="oname" style="color:var(--muted)">${esc(n)}</span><span class="otag" style="color:var(--bad)">banned</span>${act}`;
      ol.appendChild(d);
    }
  }
  const sel = $("target");
  const cur = sel.value;
  sel.innerHTML = `<option value="">— (wall)</option><option value="all">@all</option>`;
  for (const m of state.members) {
    if (m.name === state.name) continue;
    const o = document.createElement("option");
    o.value = m.name; o.textContent = "@" + m.name; sel.appendChild(o);
  }
  if ([...sel.options].some(o => o.value === cur)) sel.value = cur;
}

function avatarColor(name) {
  let h = 0; for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 60% 62%)`;
}
// WhatsApp-style short time (HH:MM); full date on hover.
function fmtTime(ts) {
  return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
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
  if (!isAdmin()) return;
  const box = $("peopleList");
  try {
    const [resRooms, resPeople, resJoin, resStock] = await Promise.all([
      fetch(`${serverBase()}/rooms`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/join-requests`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/admission-stock`, { headers: adminHeaders({}) }),
    ]);
    if (!resPeople.ok) { box.innerHTML = `<div class="callnone">liste alınamadı</div>`; return; }
    const rooms = resRooms.ok ? await resRooms.json() : [];
    const people = await resPeople.json();
    // Durable source of truth for admissions: reloaded from the server on every
    // open/reconnect, so a Master always sees pending requests even if the live
    // "wants to join" chat nudge was missed.
    state.joinRequests = resJoin.ok ? ((await resJoin.json()).pending || []) : [];
    state.admissionStock = resStock.ok ? await resStock.json() : null;

    // Ask each loca who it has barred; a ban lives in the room, not on the person.
    const bans = {};
    await Promise.all(rooms.map(async (r) => {
      try {
        const m = await fetch(`${serverBase()}/rooms/${encodeURIComponent(r.room)}/moderate`, { headers: adminHeaders({}) });
        if (m.ok) { const st = await m.json(); (st.banned || []).forEach(n => { (bans[n] ||= []).push(r.room); }); }
      } catch (e) { /* one loca failing must not blank the whole page */ }
    }));

    state.people = people;
    state.bans = bans;
    renderPeople();
  } catch (e) {
    box.innerHTML = `<div class="callnone">sunucuya ulaşılamadı</div>`;
  }
}

// Building-level admissions surface: pending join requests (the durable list,
// reloaded from the server), plus admission stock, all in the MAIN app so a
// Master approves here rather than in the hidden SSH-tunnel desk.
function joinRequestsHtml() {
  const jr = state.joinRequests || [];
  const stock = state.admissionStock;
  const gauge = stock
    ? `${stock.available} available / ${stock.total} total`
    : "stock —";
  const rows = jr.length
    ? jr.map(r =>
        `<div class="jrrow"><span class="jrwho"><b>${esc(r.name)}</b> · ${esc(r.kind)} wants to join</span>` +
        `<span class="jracts"><button data-jr-approve="${esc(r.id)}">approve</button>` +
        `<button data-jr-deny="${esc(r.id)}" class="quiet">deny</button></span></div>`).join("")
    : `<div class="jrnone">no pending join requests</div>`;
  return `<div class="jrsection">` +
    `<div class="jrhead"><span class="sectionlabel">Join requests${jr.length ? ` (${jr.length})` : ""}</span>` +
    `<span class="jrstock">admission stock: ${esc(gauge)} · <button data-jr-mint="5" class="quiet">mint 5</button></span></div>` +
    rows +
    `<div class="jrnotice" id="jrNotice"></div></div>`;
}

function renderPeople() {
  const box = $("peopleList");
  const people = state.people || [];
  const bans = state.bans || {};
  const names = new Set(people.map(p => p.name));
  Object.keys(bans).forEach(n => names.add(n));   // barred people may hold no seat

  const jrHtml = joinRequestsHtml();
  if (!names.size) {
    box.innerHTML = jrHtml + `<div class="callnone">nobody in the building</div>`;
    return;
  }
  box.innerHTML = jrHtml + [...names].sort().map(name => {
    const p = people.find(x => x.name === name);
    const glyph = p?.kind === "agent" ? "*" : ".";
    const locas = p?.locas || [];
    const barred = bans[name] || [];
    const online = p?.online ? `<span class="pon">●</span> ` : "";
    const wake = p?.runtime?.ready
      ? `<span class="pwake ok" title="runtime wake + ACK healthy">⚡</span>`
      : p?.runtime
        ? `<span class="pwake bad" title="transport online; runtime wake degraded">!</span>`
        : `<span class="pwake unknown" title="runtime wake unverified">?</span>`;
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
    const where = locas.length ? `seated: ${locas.join(", ")}` : "lobby";
    const banTxt = barred.length
      ? `<span class="pban">banned: ${esc(barred.join(", "))}</span>` : "";
    const unban = barred.map(r =>
      `<button data-unban="${esc(name)}" data-room="${esc(r)}">unban ${esc(r)}</button>`).join(" ");
    return `<div class="prow"><span class="pname">${online}${glyph}${esc(name)}${wake}${stage}</span>` +
           `<span class="pwhere">${esc(where)} ${banTxt}</span>${unban}</div>`;
  }).join("");
}

async function refreshPeopleRuntime() {
  if (!isAdmin() || state.tab !== "people") return;
  try {
    const [resPeople, resJoin, resStock] = await Promise.all([
      fetch(`${serverBase()}/residents`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/join-requests`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/admission-stock`, { headers: adminHeaders({}) }),
    ]);
    if (!resPeople.ok) return;
    state.people = await resPeople.json();
    // Keep the admissions panel live while the tab is open: a request that
    // arrives (or is approved elsewhere) shows up within the poll interval.
    if (resJoin.ok) state.joinRequests = (await resJoin.json()).pending || [];
    if (resStock.ok) state.admissionStock = await resStock.json();
    renderPeople();
  } catch (e) { /* the normal health indicator owns transport errors */ }
}
