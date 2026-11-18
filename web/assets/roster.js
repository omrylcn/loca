"use strict";
// At the table (people.js renderMembers): one line per seat in the 320px
// sidebar. The row carries the kind mark, the name and its badges; on the
// right the actions a Building admin uses daily (release, one mute toggle),
// the DM icon, and "⋯" for the rarer ones: the lead title, kick and ban.
// Kick and ban ask in words under the row before anything is sent. Every
// action keeps its route: data-mod buttons go through moderate() and
// data-lead through setLead(), as before (app.js). Who sees what follows
// the same checks as before: isAdmin() for every act, and no moderation on
// oneself or up the Building ranks (canModerateSeat).

const SEAT_ICON = (body) => `<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
const SPEAKER = '<path d="M11 5 6 9H2v6h4l5 4z"/>';
const SEAT_ICONS = {
  speaker: SEAT_ICON(`${SPEAKER}<path d="M15.5 8.5a5 5 0 0 1 0 7"/><path d="M19 5a10 10 0 0 1 0 14"/>`),
  muted: SEAT_ICON(`${SPEAKER}<path d="M15.5 8.5a5 5 0 0 1 1.3 4.6"/><path d="M3 3l18 18"/>`),
  more: SEAT_ICON('<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>'),
  lead: SEAT_ICON('<path d="M12 3 21 12 12 21 3 12z"/>'),
  kick: SEAT_ICON('<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="m16 17 5-5-5-5"/><path d="M21 12H9"/>'),
  ban: SEAT_ICON('<circle cx="12" cy="12" r="9"/><path d="m5.7 5.7 12.6 12.6"/>'),
};

// The kick or ban waiting for a yes, and the seat whose menu is open.
let seatAsk = null;
let seatMenuFor = null;

function seatPerson(name) {
  return [...(state.members || []), ...(state.seatedAway || []).map(p => ({ name: p.name, type: p.kind, away: true }))]
    .find(m => m.name === name) || null;
}
function seatMayModerate(m) {
  return isAdmin() && !!m && m.name !== state.name && canModerateSeat(m);
}

// One seat's line. `muted` and `isLead` come from renderMembers.
function seatRowHtml(m, { isMe, isLead, muted }) {
  const glyph = isLead ? "◆" : (m.type === "agent" ? "*" : ".");
  const card = cardOf(m);
  // A name cut short by the ellipsis can still be read whole on hover.
  const nameTitle = ` title="${card?.full_name ? `${esc(card.full_name)} (@${esc(m.name)})` : esc(m.name)}"`;
  const mod = seatMayModerate(m);
  // The caretaker's title, as on its lines (chat.js roleTagsFor): only on a
  // seat the server verified.
  const caretaker = m.principal_id && (state.locaAgents || []).includes(m.name)
    ? `<span class="caretakertag" title="the Building's caretaker">caretaker</span>` : "";
  // Muted shows on the toggle where there is one; anyone else sees a mark.
  const mutedMark = muted && !mod
    ? `<span class="mflag" role="img" title="muted in this loca" aria-label="muted">${SEAT_ICONS.muted}</span>` : "";
  const badges = (m.away ? `<span class="seatstate" title="holds a seat here, not connected now">away</span>` : "")
    + rankBadge(m.role)
    + (isLead ? `<span class="leadtag" title="loca lead">lead</span>` : "")
    + caretaker
    + (typeof taskBadgeFor === "function" ? taskBadgeFor(m.name, m.principal_id || "") : "")
    + mutedMark
    + kitVersionHtml(m, "seat");

  // Anyone at the table may speak to one person privately, never from inside
  // a direct message. An empty slot keeps the icons in columns.
  const slot = `<span class="seatslot" aria-hidden="true"></span>`;
  const dm = !isMe && !isDm(state.room)
    ? `<button type="button" class="dmbtn" data-dm="${esc(m.name)}" aria-label="Message ${esc(m.name)} privately" data-tip="Message ${esc(m.name)}" data-tip-detail="A private conversation: only the two of you see it.">${DM_ICON}</button>`
    : slot;
  let acts = dm;
  if (isAdmin()) {
    // Release first: finishing a job is the common case, and it must not
    // look like a punishment.
    const release = mod
      ? `<button type="button" class="release" data-mod="release" data-n="${esc(m.name)}" aria-label="Release ${esc(m.name)}" data-tip="Release" data-tip-detail="Work done: leaves this loca; Building membership stays.">release</button>`
      : "";
    const mute = mod
      ? `<button type="button" class="seatbtn mutebtn" data-mod="${muted ? "unmute" : "mute"}" data-n="${esc(m.name)}" aria-pressed="${muted}" aria-label="Mute ${esc(m.name)}" data-tip="${muted ? "Unmute" : "Mute"}" data-tip-detail="${muted ? `${esc(m.name)} is muted here; press to let them speak.` : `${esc(m.name)} stays seated but cannot speak here.`}">${muted ? SEAT_ICONS.muted : SEAT_ICONS.speaker}</button>`
      : slot;
    const open = seatMenuFor === m.name;
    const more = `<button type="button" class="seatbtn seatmore" data-seat-menu="${esc(m.name)}" aria-haspopup="menu" aria-expanded="${open}" aria-controls="seatMenu" aria-label="More for ${esc(m.name)}" data-tip="More" data-tip-detail="Lead, kick and ban">${SEAT_ICONS.more}</button>`;
    acts = release + mute + dm + more;
  }
  return `<span class="glyph ${m.type}${isLead ? " lead" : ""}" title="${isLead ? "loca lead" : m.type}">${glyph}</span>`
    + `<span class="oname"${nameTitle}>${personLabelHtml(m.name, card)}${isMe ? " (you)" : ""}</span>`
    + (badges ? `<span class="obadges">${badges}</span>` : "")
    + `<span class="seatacts">${acts}</span>`;
}

// The question under a seat, when kick or ban was chosen for it.
function seatAskHtml(name) {
  const ask = seatAsk;
  if (!ask || ask.name !== name || ask.room !== state.room || ask.gen !== state.buildingGen) return "";
  const question = ask.action === "ban"
    ? `Ban <b>${esc(name)}</b> from this loca? Their seat ends and they cannot enter it again until someone unbans them.`
    : `Kick <b>${esc(name)}</b> from this loca? Their connection closes and their invite stops working; they can be called in again.`;
  const yes = ask.action === "ban" ? "Yes, ban" : "Yes, kick";
  return `<div class="seatask" role="alertdialog" aria-label="${ask.action === "ban" ? "Ban" : "Kick"} ${esc(name)}"><span>${question}</span>`
    + `<span class="seataskacts"><button type="button" class="btn" data-seat-no>No</button>`
    + `<button type="button" class="btn danger" data-seat-yes data-mod="${ask.action}" data-n="${esc(name)}">${yes}</button></span></div>`;
}

// ---- The ⋯ menu ------------------------------------------------------------
// One menu on <body>, fixed to the screen: the sidebar's own scrolling and
// edges never cut it. It opens below its button, or above it when the
// button sits near the bottom of the window.

function seatMenuEl() {
  let menu = $("seatMenu");
  if (!menu) {
    menu = document.createElement("div");
    menu.id = "seatMenu";
    menu.className = "seatmenu hidden";
    menu.setAttribute("role", "menu");
    document.body.appendChild(menu);
  }
  return menu;
}

function seatMenuItemsHtml(name) {
  const m = seatPerson(name);
  if (!m || !isAdmin()) return "";
  const lead = state.settings?.lead === name
    ? `<button type="button" role="menuitem" class="seatmenuitem" data-lead="">${SEAT_ICONS.lead}<span>End lead</span></button>`
    : `<button type="button" role="menuitem" class="seatmenuitem" data-lead="${esc(name)}">${SEAT_ICONS.lead}<span>Make lead</span></button>`;
  const moderation = seatMayModerate(m)
    ? `<button type="button" role="menuitem" class="seatmenuitem danger" data-seat-ask="kick" data-n="${esc(name)}">${SEAT_ICONS.kick}<span>Kick…</span></button>`
      + `<button type="button" role="menuitem" class="seatmenuitem danger" data-seat-ask="ban" data-n="${esc(name)}">${SEAT_ICONS.ban}<span>Ban…</span></button>`
    : "";
  return lead + moderation;
}

function seatMenuTrigger() {
  return seatMenuFor === null ? null
    : [...document.querySelectorAll("#onlineList [data-seat-menu]")].find(b => b.dataset.seatMenu === seatMenuFor) || null;
}

function placeSeatMenu(menu, trigger) {
  const r = trigger.getBoundingClientRect();
  const w = menu.offsetWidth, h = menu.offsetHeight;
  const left = Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8));
  let top = r.bottom + 4;
  const up = top + h > window.innerHeight - 8;
  if (up) top = Math.max(8, r.top - 4 - h);
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;
  menu.classList.toggle("up", up);
}

function openSeatMenu(trigger) {
  const name = trigger.dataset.seatMenu;
  const items = seatMenuItemsHtml(name);
  if (!items) return;
  const menu = seatMenuEl();
  seatMenuFor = name;
  menu.innerHTML = items;
  menu.setAttribute("aria-label", `Actions for ${name}`);
  menu.classList.remove("hidden");
  trigger.setAttribute("aria-expanded", "true");
  placeSeatMenu(menu, trigger);
  menu.querySelector("[role='menuitem']")?.focus();
}

function closeSeatMenu(returnFocus = false) {
  if (seatMenuFor === null) return;
  const trigger = seatMenuTrigger();
  seatMenuFor = null;
  seatMenuEl().classList.add("hidden");
  trigger?.setAttribute("aria-expanded", "false");
  if (returnFocus) trigger?.focus();
}

// After the roster is drawn again (a member came or went, the lead moved):
// keep an open menu on its seat with what is allowed now, or close it.
function syncSeatMenu() {
  if (seatMenuFor === null) return;
  const trigger = seatMenuTrigger();
  const items = trigger ? seatMenuItemsHtml(seatMenuFor) : "";
  if (!items) { closeSeatMenu(); return; }
  const menu = seatMenuEl();
  if (menu.innerHTML !== items) {
    const focused = menu.contains(document.activeElement);
    menu.innerHTML = items;
    if (focused) menu.querySelector("[role='menuitem']")?.focus();
  }
  placeSeatMenu(menu, trigger);
}

function askSeat(action, name) {
  seatAsk = { action, name, room: state.room, gen: state.buildingGen };
  renderMembers();
  document.querySelector("#onlineList .seatask [data-seat-no]")?.focus();
}

$("onlineList").addEventListener("click", (event) => {
  const more = event.target.closest("[data-seat-menu]");
  if (more) {
    const same = seatMenuFor === more.dataset.seatMenu;
    closeSeatMenu();
    if (!same) openSeatMenu(more);
    return;
  }
  if (event.target.closest("[data-seat-no]")) {
    const name = seatAsk?.name;
    seatAsk = null;
    renderMembers();
    if (name) [...document.querySelectorAll("#onlineList [data-seat-menu]")].find(b => b.dataset.seatMenu === name)?.focus();
    return;
  }
  // Yes: the button carries data-mod, which app.js sends through moderate().
  if (event.target.closest("[data-seat-yes]")) seatAsk = null;
});

seatMenuEl().addEventListener("click", (event) => {
  const item = event.target.closest("[role='menuitem']");
  if (!item) return;
  closeSeatMenu();
  if (item.dataset.seatAsk) { askSeat(item.dataset.seatAsk, item.dataset.n); return; }
  if ("lead" in item.dataset) setLead(item.dataset.lead || null);
});

seatMenuEl().addEventListener("keydown", (event) => {
  const items = [...seatMenuEl().querySelectorAll("[role='menuitem']")];
  const at = items.indexOf(document.activeElement);
  if (event.key === "Escape") { event.preventDefault(); closeSeatMenu(true); }
  else if (event.key === "ArrowDown") { event.preventDefault(); items[(at + 1) % items.length]?.focus(); }
  else if (event.key === "ArrowUp") { event.preventDefault(); items[(at - 1 + items.length) % items.length]?.focus(); }
  else if (event.key === "Home") { event.preventDefault(); items[0]?.focus(); }
  else if (event.key === "End") { event.preventDefault(); items[items.length - 1]?.focus(); }
  else if (event.key === "Tab") closeSeatMenu();
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && seatMenuFor !== null) closeSeatMenu(true);
});
document.addEventListener("pointerdown", (event) => {
  if (seatMenuFor === null) return;
  if (event.target.closest("#seatMenu, [data-seat-menu]")) return;
  closeSeatMenu();
});
// The sidebar scrolls under a fixed menu: close it rather than leave it
// floating beside another seat.
document.addEventListener("scroll", (event) => {
  if (seatMenuFor !== null && !seatMenuEl().contains(event.target)) closeSeatMenu();
}, true);
window.addEventListener("resize", () => closeSeatMenu());
