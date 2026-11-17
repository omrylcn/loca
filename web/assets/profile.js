"use strict";
// Server-derived identity, composed Building/Loca roles, and personal access.

function profileRoleLabel(role) {
  return String(role || "member").replaceAll("_", " ").toUpperCase();
}

// One's roles in a loca, as shown. Operator is a seat a person is appointed
// to; the Master holds its authority in every loca by rank, not by
// appointment (PRINCIPLES): that is shown as the rank, never as OPERATOR.
function locaRoleLabels(loca) {
  const roles = loca?.roles || [];
  return roles
    .filter(role => role !== "participant" || roles.length === 1)
    .map(role => role === "operator" && loca?.operator_source === "master" ? "MASTER" : profileRoleLabel(role));
}

function profileKindLabel(kind) {
  return kind === "agent" ? "Agent" : "Human";
}

const APPEARANCE_LABELS = {
  theme: { dark: "Dark", ember: "Ember", contrast: "High contrast", night: "Night", light: "Light", paper: "Paper" },
  accent: {
    teal: "Teal", blue: "Blue", indigo: "Indigo", violet: "Violet", rose: "Rose", orange: "Orange",
    amber: "Amber", green: "Green", sky: "Sky", custom: "Custom colour",
  },
  density: { compact: "Compact", comfortable: "Comfortable", spacious: "Spacious" },
  font_size: { small: "Small", normal: "Normal", large: "Large", larger: "Larger" },
  font_family: { mono: "Mono", sans: "Sans", serif: "Serif" },
  layout: { bubbles: "Cards", flat: "Flat" },
  time_format: { auto: "Device clock", h24: "24-hour", h12: "12-hour", relative: "Relative" },
  measure: { narrow: "Narrow", normal: "Medium", wide: "Wide" },
  own_tint: { on: "Tinted", off: "Plain" },
  motion: { system: "Follow device", reduced: "Reduced" },
};
// The two named groups of the look, each a list of rows: [key, title, hint].
const APPEARANCE_GROUPS = [
  ["Theme", "profileSecTheme", [
    ["theme", "Theme", ""], ["accent", "Accent", ""], ["font_family", "Font", ""],
    ["font_size", "Text", ""], ["density", "Density", ""],
  ]],
  ["Reading", "profileSecReading", [
    ["layout", "Messages", "Cards put yours on the right; flat is one column."],
    ["time_format", "Times", ""],
    ["measure", "Line length", ""],
    ["own_tint", "Your messages", ""],
    ["motion", "Motion", "Reduced stills transitions everywhere in the app."],
  ]],
];

function currentAppearance() {
  return state.appearance || cachedAppearance();
}

// One radio as a chip. A theme is drawn as a small picture of itself, an
// accent as its colour, a font in its own face: the chip is its preview.
function appearanceOptionHtml(key, value, prefs) {
  const name = APPEARANCE_LABELS[key][value];
  const checked = prefs[key] === value ? " checked" : "";
  let face = `<span>${name}</span>`;
  let kind = "";
  if (key === "accent") {
    kind = " swatchopt";
    face = `<span class="swatch ${value}" title="${name}" aria-hidden="true"></span><span class="visuallyhidden">${name}</span>`;
  } else if (key === "theme") {
    kind = " themeopt";
    face = `<span class="themecard" data-theme-sample="${value}"><span class="tcpage" aria-hidden="true">` +
      `<i class="tcline"></i><i class="tcline short"></i><i class="tcdot"></i></span><span class="tcname">${name}</span></span>`;
  } else if (key === "font_family") {
    face = `<span class="fontface ${value}">${name}</span>`;
  }
  return `<label class="appearanceopt${kind}"><input type="radio" name="appearance-${key}"` +
    ` data-appearance="${key}" value="${value}"${checked} />${face}</label>`;
}

function appearanceRowHtml(key, title, hint, prefs) {
  const labelId = `appearanceLabel-${key}`;
  const options = APPEARANCE_CHOICES[key].map(value => appearanceOptionHtml(key, value, prefs)).join("");
  const custom = key === "accent" ? customAccentHtml(prefs) : "";
  return `<div class="appearancerow" role="radiogroup" aria-labelledby="${labelId}">` +
    `<span id="${labelId}">${title}</span><div class="appearanceopts">${options}${custom}</div>` +
    (hint ? `<small class="appearancehint">${hint}</small>` : "") + `</div>`;
}

// A colour of one's own. The browser keeps it readable on the theme
// (theme.js readableAccent) and says so when it had to move it.
function customAccentHtml(prefs) {
  const value = prefs.accent_custom || "#2ee6c8";
  return `<label class="accentpick" title="Choose a custom accent colour">` +
    `<input type="color" id="accentCustomInput" value="${esc(value)}" aria-label="Custom accent colour" />` +
    `<span>Pick…</span></label>`;
}

function customAccentNote(prefs) {
  if (prefs.accent !== "custom" || !prefs.accent_custom) return "";
  const used = readableAccent(prefs.accent_custom, prefs.theme);
  return used.adjusted
    ? `Your colour ${prefs.accent_custom} was ${LIGHT_THEMES.includes(prefs.theme) ? "darkened" : "lightened"} to ${used.accent} so text in it stays readable (4.5:1) on ${APPEARANCE_LABELS.theme[prefs.theme]}.`
    : `${prefs.accent_custom} reads well on ${APPEARANCE_LABELS.theme[prefs.theme]}.`;
}

// The sample: two lines drawn with the chat's own rules (layout, font, text
// size, line length, your tint, the time style), so each click previews here
// what it changes in the loca.
function appearanceSampleHtml() {
  const now = Date.now();
  const me = state.profile?.principal;
  const card = me ? cardOf(me) : null;
  const name = state.name || "you";
  return `<div class="row mentioned"><div class="bubble mentioned"><div class="line"><span class="glyph user">.</span>` +
      `<span class="sender user">ada</span><span class="time">${esc(fmtMessageTime(now - 4 * 60_000))}</span></div>` +
      `<div class="body markdown chatmarkdown">Morning <span class="mention">@${esc(name)}</span>, the plan is in the wiki.</div></div></div>` +
    `<div class="row mine"><div class="bubble"><div class="line"><span class="glyph user">.</span>` +
      `<span class="sendercard">${avatarHtml(card, name, "xs")}${card?.full_name ? `<bdi class="fullname">${esc(card.full_name)}</bdi>` : ""}</span>` +
      `<span class="sender user">${esc(name)}</span><span class="time">${esc(fmtMessageTime(now - 60_000))}</span></div>` +
      `<div class="body markdown chatmarkdown">The table is set. 0O 1lI</div></div></div>`;
}

// Personal look: real radio groups, applied instantly, saved per principal.
function profileAppearanceHtml() {
  const prefs = currentAppearance();
  const groups = APPEARANCE_GROUPS.map(([title, id, rows]) =>
    `<section class="psec" id="${id}" aria-labelledby="${id}-h"><h3 class="psechead" id="${id}-h">${title}</h3>` +
    rows.map(([key, label, hint]) => appearanceRowHtml(key, label, hint, prefs)).join("") +
    (id === "profileSecTheme" ? `<div class="accentnote" id="accentNote" role="status">${esc(customAccentNote(prefs))}</div>` : "") +
    `</section>`).join("");
  const sample = `<div class="appearancepreview" id="appearancePreview" aria-hidden="true">${appearanceSampleHtml()}</div>`;
  return `<div class="profileappearance" id="profileAppearance">` +
    `<div class="samplecard"><small>Preview</small>${sample}</div>${groups}` +
    `<div class="appearancefoot"><button type="button" class="pbtn ghost" id="appearanceReset">Reset look to defaults</button>` +
    `<div class="appearancestatus" id="appearanceStatus" role="status"></div></div></div>`;
}

function syncAppearanceInputs() {
  const prefs = currentAppearance();
  document.querySelectorAll("input[data-appearance]").forEach(input => {
    input.checked = prefs[input.dataset.appearance] === input.value;
  });
  const picker = $("accentCustomInput");
  if (picker && prefs.accent_custom) picker.value = prefs.accent_custom;
  refreshAppearanceSample();
}

// What follows a change of look: the sample, the custom-accent note, and the
// times already in the feed.
function refreshAppearanceSample() {
  const sample = $("appearancePreview");
  if (sample) sample.innerHTML = appearanceSampleHtml();
  const note = $("accentNote");
  if (note) note.textContent = customAccentNote(currentAppearance());
  if (typeof refreshFeedTimes === "function") refreshFeedTimes();
}

function setAppearanceStatus(text) {
  const status = $("appearanceStatus");
  if (status) status.textContent = text;
}

// Server copy wins over the local cache unless the person changed something
// while the read was in flight: their newer click is already being saved.
let appearanceEdits = 0;
async function fetchAppearance() {
  const principal = state.principalId;
  if (!principal) return;
  const editsAtStart = appearanceEdits;
  try {
    const response = await fetch(`${serverBase()}/profile/preferences`, { headers: adminHeaders({}) });
    if (!response.ok) return;
    const prefs = await response.json();
    if (editsAtStart !== appearanceEdits || principal !== state.principalId) return;
    state.appearance = applyAppearance(prefs);
    state.appearancePrincipal = principal;
    cacheAppearance(state.appearance);
    syncAppearanceInputs();
  } catch (error) {}
}

let appearanceSaveSequence = 0;
async function saveAppearance(key, value) {
  return saveAppearancePatch({ [key]: value });
}
// Apply at once, keep on this device, then save to the profile. A failed
// save says so and keeps the look here.
async function saveAppearancePatch(patch) {
  appearanceEdits++;
  state.appearance = applyAppearance({ ...currentAppearance(), ...patch });
  cacheAppearance(state.appearance);
  refreshAppearanceSample();
  const sequence = ++appearanceSaveSequence;
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/profile/preferences`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(patch),
    });
    if (!response.ok) throw new Error(await response.text());
    const saved = await response.json();
    if (sequence !== appearanceSaveSequence) return;
    state.appearance = applyAppearance(saved);
    cacheAppearance(state.appearance);
    setAppearanceStatus("");
    refreshAppearanceSample();
  } catch (error) {
    if (sequence === appearanceSaveSequence && !buildingStale(gen)) setAppearanceStatus("Not saved to your profile; kept on this device.");
  }
}

function resetAppearance() {
  saveAppearancePatch({ ...APPEARANCE_DEFAULTS });
  syncAppearanceInputs();
}

// The account menu's own part: the server this page is signed in to, and the
// one way out, Sign out (app.js signOutHere). Shown whatever else the profile
// could or could not say, so leaving never depends on the profile loading.
function profileServerHtml() {
  return `<div class="profilesession profileserver"><small>Server</small>` +
    `<span class="pserverline"><span id="accountServer">${esc(state.server || serverBase())}</span>` +
    `<button type="button" class="pbtn ghost small" id="accountServerCopy" aria-label="Copy the server address">Copy</button></span></div>`;
}
function profileSignOutHtml() {
  const master = state.profile?.building_role === "master";
  return `<div class="profilesignout"><button type="button" class="leavebtn" id="leaveBtn">Sign out</button>` +
    `<div class="leaveconfirm hidden" id="leaveConfirm" role="alertdialog" aria-label="Sign out of this browser">` +
      `Sign out of this browser? Your loca sessions here close. Your membership and your key stay; ` +
      (master ? "a fresh pairing code signs you in again." : "your key signs you in again.") +
      `<div class="choices"><button type="button" class="no" id="leaveNo">Cancel</button>` +
      `<button type="button" class="yes" id="leaveYes">Sign out</button></div></div></div>`;
}

// The account menu is redrawn whenever the profile is read again (a loca
// switch, a lead change, every journal entry while the Building view is open).
// A redraw for the same person keeps it as they had it: open, and with the
// Sign out confirmation still asked, so a click aimed at it is not lost to a
// menu that shut under the pointer. A new person (generation) starts closed.
function accountMenuState(box) {
  if (box.dataset.gen !== String(state.buildingGen)) return { open: false, confirming: false };
  const menu = box.querySelector("details.profilemenu");
  return {
    open: !!menu?.open,
    confirming: !!menu?.open && !!box.querySelector("#leaveConfirm:not(.hidden)"),
  };
}
function restoreAccountMenu(box, kept) {
  box.dataset.gen = String(state.buildingGen);
  if (!kept.open) return;
  const menu = box.querySelector("details.profilemenu");
  if (menu) menu.open = true;
  if (kept.confirming) $("leaveConfirm")?.classList.remove("hidden");
}

// While the person has the account menu open it is theirs: a name half
// typed, an icon picked but not saved, the one-time view of a new key, an
// opened section, the caret. A redraw that only follows the server (a
// journal entry, a lead change, a profile read again) would wipe all of it,
// so it waits until the menu closes. What the person did themselves (saved,
// renamed, made or revoked a key) redraws at once: those paths pass `now`.
// A new person (generation) is always drawn.
let profileRedrawWaiting = false;
function accountMenuInUse(box) {
  return box.dataset.gen === String(state.buildingGen)
    && !!box.querySelector("details.profilemenu")?.open;
}
function profileRedrawMustWait(box, now) {
  if (now || !accountMenuInUse(box)) {
    profileRedrawWaiting = false;
    return false;
  }
  profileRedrawWaiting = true;
  return true;
}
// `toggle` does not bubble: caught on the way down. Opened, the sheet starts
// below the name that opened it, so that name stays in reach to close it.
$("whoami").addEventListener("toggle", event => {
  if (!event.target.matches?.("details.profilemenu")) return;
  if (event.target.open) {
    const bottom = event.target.querySelector("summary")?.getBoundingClientRect().bottom || 0;
    event.target.style.setProperty("--profile-top", `${Math.round(bottom + 6)}px`);
    return;
  }
  if (profileRedrawWaiting) renderProfile();
}, true);

// ---- Small words the panel uses ------------------------------------------
function profileDate(value) {
  return new Date(Number(value)).toLocaleString([], {
    month: "short", day: "numeric", year: "numeric", hour: "2-digit", minute: "2-digit",
  });
}
function profileDay(value) {
  return new Date(Number(value)).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
}
// "in 16 days", "3 hours ago": how far a moment is from now, in plain words.
function profileFromNow(value, now = Date.now()) {
  const diff = Number(value) - now;
  const ahead = diff >= 0;
  const minutes = Math.round(Math.abs(diff) / 60_000);
  const [n, unit] = minutes < 1 ? [0, ""] : minutes < 60 ? [minutes, "minute"]
    : minutes < 60 * 48 ? [Math.round(minutes / 60), "hour"] : [Math.round(minutes / 1440), "day"];
  if (!unit) return ahead ? "in a moment" : "just now";
  const words = `${n} ${unit}${n === 1 ? "" : "s"}`;
  return ahead ? `in ${words}` : `${words} ago`;
}

// The shell every state of the menu shares: the summary (who is signed in)
// and the panel (head, body, Sign out).
function profileShellHtml(summary, body) {
  return `<details class="profilemenu">${summary}` +
    `<div class="profilepanel" role="group" aria-label="Your profile">` +
      `<div class="profilepanelhead"><span>Profile</span><small>Identity &amp; access</small>` +
      `<button type="button" class="profileclose" id="profileClose" aria-label="Close your profile">×</button></div>` +
      `${body}${profileSignOutHtml()}</div></details>`;
}

// Before a profile is read, or when it cannot be: the name, the server and
// Sign out, nothing more.
function profileFallbackHtml(text) {
  const summary = `<summary><span class="profileidentity"><span class="profileglyph">${avatarHtml(null, state.name, "sm")}</span>` +
    `<span><b>${esc(state.name)}</b><small>connected as ${esc(state.name)}</small></span></span>` +
    `<span class="profilechevron" aria-hidden="true">›</span></summary>`;
  return profileShellHtml(summary, `<div class="profileempty">${text}</div>${profileServerHtml()}`);
}

function profileSummaryHtml(principal) {
  const card = cardOf(principal);
  // The avatar stands in the glyph's place; the label beside it is the name.
  const label = card ? { ...card, icon: "", avatar_color: "" } : null;
  return `<summary aria-label="Open your account menu">` +
    `<span class="profileidentity"><span class="profileglyph">${avatarHtml(card, principal.display_name, "sm")}</span>` +
      `<span><b>${personLabelHtml(principal.display_name, label)}</b><small>You · ${profileKindLabel(principal.kind)}</small></span></span>` +
    `<span class="profilechevron" aria-hidden="true">›</span></summary>`;
}

// The sections, in reading order. On a wide screen the look (Theme,
// Reading) stands in a second column beside who and where you are.
const PROFILE_NAV = [
  ["profileSecYou", "You"], ["profileSecPlace", "Where you are"], ["profileSecAccess", "Session and access"],
  ["profileSecTheme", "Theme"], ["profileSecReading", "Reading"],
];
function profileNavHtml() {
  return `<nav class="profilenav" aria-label="Profile sections">` +
    PROFILE_NAV.map(([id, label]) => `<button type="button" class="pnavbtn" data-profile-jump="${id}">${label}</button>`).join("") +
    `</nav>`;
}

function renderProfile(now = false) {
  const box = $("whoami");
  if (profileRedrawMustWait(box, now)) return;
  const profile = state.profile;
  const kept = accountMenuState(box);
  if (!profile?.principal) {
    box.innerHTML = profileFallbackHtml("Profile is available after you take a seat.");
    restoreAccountMenu(box, kept);
    box.classList.add("on");
    return;
  }
  const principal = profile.principal;
  const body = profileNavHtml() + `<div class="profilebody"><div class="profilecol">` +
    profileYouHtml(profile, principal) + profilePlaceHtml(profile) + profileAccessHtml(profile) +
    `</div><div class="profilecol">${profileAppearanceHtml()}</div></div>`;
  box.innerHTML = profileShellHtml(profileSummaryHtml(principal), body);
  restoreAccountMenu(box, kept);
  box.classList.add("on");
}

// ---- You: who you are here, and what is yours to change -------------------
function profileYouHtml(profile, principal) {
  const card = cardOf(principal);
  return `<section class="psec" id="profileSecYou" aria-labelledby="profileSecYou-h">` +
    `<h3 class="psechead" id="profileSecYou-h">You</h3>` +
    `<div class="profilehero"><span class="profileavatar" id="profileAvatarPreview">${avatarHtml(card, principal.display_name, "xl")}</span>` +
      `<div class="profileconnected">` +
        `<div class="phfull" id="profileHeroName">${card?.full_name ? `<bdi>${esc(card.full_name)}</bdi>` : ""}</div>` +
        `<div class="phat">@<b>${esc(principal.display_name)}</b>${rankBadge(profile.building_role)}</div>` +
        `<div class="phkind">You · ${profileKindLabel(principal.kind)}</div>` +
      `</div></div>` +
    profileCardHtml(principal) +
    profileRenameHtml(profile, principal) +
    `</section>`;
}

// ---- Where you are: this Building, this loca, your floors -----------------
// Each sentence restates what the server answered (building_role,
// loca.roles, operator_source, building_management) in PRINCIPLES' words;
// none of it grants anything.
const BUILDING_RANK_WORDS = {
  master: "You own this Building: you admit members, issue invitations and have the final word.",
  smaster: "Second master: you admit members, issue invitations and manage locas. Ending a membership is the Master's.",
  member: "A member of this Building: you sit at the locas you are invited to.",
};
function locaRoleSentence(loca) {
  const roles = loca?.roles || [];
  const words = [];
  if (loca?.operator_source === "master") {
    words.push("Your rank holds this loca's management authority; it is not an appointed seat.");
  } else if (loca?.operator_source === "appointed") {
    words.push("Appointed here: you set the mode and turn order, and you moderate.");
  }
  if (roles.includes("lead")) words.push("As lead you watch the room, advise and report; you do not assign or moderate.");
  if (!words.length && roles.includes("participant")) words.push("You take part at this table.");
  return words.join(" ");
}
function locaSeatText(room) {
  if (!room) return "";
  if (isDm(room)) return "A direct message";
  return isWatchingDoor() ? "Watching from the door" : "Seated at the table";
}
function profilePlaceHtml(profile) {
  const loca = profile.loca;
  const room = loca?.room || "";
  const labels = locaRoleLabels(loca);
  const source = loca?.operator_source === "appointed" ? "appointed" : "";
  const management = loca?.building_management ? "Building management access" : "";
  const role = profile.building_role || "member";
  const summary = (state.rooms || []).find(r => r.room === room);
  const floor = summary?.group ? floorName(summary.group) : room ? "Ground floor" : "";
  const locaCard = room
    ? `<div class="placename">${esc(roomLabel(room))}</div>` +
      `<div class="placemeta">${esc([locaSeatText(room), floor].filter(Boolean).join(" · "))}</div>` +
      `<b class="role loca-role">${esc(labels.length ? labels.join(" · ") : "PARTICIPANT")}</b>` +
      (source ? `<em>${source}</em>` : "") + (management ? `<em>${management}</em>` : "") +
      `<p>${esc(locaRoleSentence(loca))}</p>`
    : `<div class="placename muted">No loca open</div><p>Open a loca from the sidebar to see your seat there.</p>`;
  return `<section class="psec" id="profileSecPlace" aria-labelledby="profileSecPlace-h">` +
    `<h3 class="psechead" id="profileSecPlace-h">Where you are</h3>` +
    profileServerHtml() +
    `<div class="profileroles">` +
      `<span class="placecard placebuilding"><small>Building</small><b class="role building-${esc(role)}">${profileRoleLabel(role)}</b>` +
        `<p>${esc(BUILDING_RANK_WORDS[role] || "")}</p></span>` +
      `<span class="placecard placeloca"><small>This Loca</small>${locaCard}</span>` +
    `</div>` + profileGroupsHtml(profile) + `</section>`;
}

// A floor reads as a floor, as in the sidebar: "Product floor".
function floorName(name) {
  return /floor\s*$/i.test(name || "") ? name : `${name} floor`;
}

// The floors I am on. Everyone is on the Ground floor; a floor adds the
// locas only its people see.
function profileGroupsHtml(profile) {
  const groups = Array.isArray(profile.groups) ? profile.groups : [];
  const chips = [`<span class="grpchip ground">Ground floor</span>`]
    .concat(groups.map(g => {
      const { cls, style } = typeof floorColourAttrs === "function" ? floorColourAttrs(g.id) : { cls: "", style: "" };
      return `<span class="grpchip${cls}"${style}>${GROUP_ICON || ""}${esc(floorName(g.name))}</span>`;
    })).join("");
  const note = groups.length ? "" : `<small class="pnowhere">Ground floor only: no floor has added you yet.</small>`;
  return `<div class="profilegroups"><small>Your floors</small><span class="grpchips">${chips}</span>${note}</div>`;
}

// ---- Session and access: how you are signed in, and your keys -------------
function profileSessionHtml(profile) {
  const session = profile.session;
  const credentials = state.credentials || [];
  const current = credentials.find(c => c.current) || credentials.find(c => c.id === session?.credential_id);
  const using = current ? `<b>${esc(current.label)}</b>` : "your key";
  const who = `connected as <b>${esc(profile.principal.display_name)}</b>`;
  if (!session) {
    return `<div class="profilesession sessioncard"><small>Session</small>` +
      `<span>Direct credential, ${who} with ${using}: it has no session end of its own.</span></div>`;
  }
  const until = session.expires_at
    ? `until <b>${esc(profileDate(session.expires_at))}</b> <em>(${esc(profileFromNow(session.expires_at))})</em>`
    : "with no expiry";
  return `<div class="profilesession sessioncard"><small>Session</small>` +
    `<span>Bounded session, ${who} through ${using}, ${until}.</span></div>`;
}
function credentialRowHtml(credential) {
  const stateText = credential.revoked_at ? "revoked" : credential.current ? "current" : "active";
  const facts = [];
  if (credential.created_at) facts.push(`Added ${profileDay(credential.created_at)}`);
  facts.push(credential.last_used_at ? `last used ${profileFromNow(credential.last_used_at)}` : "not used yet");
  if (credential.revoked_at) facts.push(`revoked ${profileDay(credential.revoked_at)}`);
  const what = credential.current ? "This browser's key" : credential.root_recovery ? "Recovery key" : "Access key";
  const action = credential.root_recovery || credential.revoked_at ? ""
    : `<button type="button" class="pbtn danger small" data-revoke-credential="${esc(credential.id)}"` +
      ` data-current="${credential.current ? "1" : "0"}">` +
      `${credential.current ? "revoke & end session" : "revoke"}</button>`;
  return `<div class="credentialrow ${credential.revoked_at ? "revoked" : ""}">` +
    `<span><b>${esc(credential.label)}</b><small>${esc(what)} · ${esc(facts.join(" · "))}</small></span>` +
    `<span class="credentialstate ${stateText}">${esc(stateText)}${credential.root_recovery ? " · recovery" : ""}</span>${action}</div>`;
}
function profileAccessHtml(profile) {
  const credentials = state.credentials || [];
  const active = credentials.filter(c => !c.revoked_at).length;
  const rows = credentials.map(credentialRowHtml).join("");
  return `<section class="psec" id="profileSecAccess" aria-labelledby="profileSecAccess-h">` +
    `<h3 class="psechead" id="profileSecAccess-h">Session and access <span class="psecount">${active} active</span></h3>` +
    profileSessionHtml(profile) +
    `<div class="credentiallist">${rows || '<div class="credentialnone">No access keys listed.</div>'}</div>` +
    `<details class="profileaccess"><summary>New key for another device</summary>` +
      `<div class="credentialcreate"><label class="visuallyhidden" for="credentialLabel">Label for the new key</label>` +
      `<input id="credentialLabel" maxlength="64" placeholder="Label, e.g. MacBook" />` +
      `<button type="button" class="pbtn" id="credentialCreate">Create key</button></div>` +
      `<div class="credentialsecret hidden" id="credentialSecret" role="status"></div>` +
    `</details></section>`;
}

// ---- A person's card: who they are and their groups ----------------------
// Opened from a name in the Building view. The founder of a group (or the
// Master) adds and removes the person here; everyone else only reads. Built
// from the groups the viewer may see, so a card never reveals a hidden one.
let personCardName = null;
let personCardReturnFocus = null;

function personCardProfile(name) {
  return (state.profilesList || []).find(p => p.display_name === name) || null;
}

function renderPersonCard() {
  const name = personCardName;
  if (!name) return;
  const person = personCardProfile(name);
  const resident = (state.people || []).find(p => p.name === name);
  const kind = person?.kind === "agent" || resident?.kind === "agent" ? "agent" : "human";
  const role = person?.role || "member";
  $("personCardTitle").innerHTML = `${personLabelHtml(name, cardOf(person) || cardOf(resident))}${rankBadge(role)}<small>${kind === "agent" ? "Agent" : "Human"}</small>`;
  const theirs = groupsOfPerson(name);
  const chips = theirs.length ? theirs.map(g => {
    const founder = !g.owned_by_master && person && g.owner_id === person.id;
    const remove = g.can_manage && !founder && person
      ? `<button type="button" class="btn danger grpremove" data-card-remove="${esc(g.id)}" aria-label="Remove ${esc(name)} from ${esc(g.name)}" title="Remove from ${esc(g.name)}">${GROUP_REMOVE_ICON}<span>Remove</span></button>`
      : "";
    return `<div class="grpmember"><span class="pname">${GROUP_ICON}<b>${esc(g.name)}</b>${founder ? `<span class="pkind">founder</span>` : ""}</span><span class="pacts">${remove}</span></div>`;
  }).join("") : `<div class="bbempty">Ground floor only.</div>`;
  let add = "";
  if (role === "master") {
    add = `<p class="extnote">The Master sees every floor and is never a member of one.</p>`;
  } else if (person && person.id !== state.principalId) {
    const open = (state.groups || []).filter(g =>
      g.can_manage && !g.archived && !(g.members || []).some(m => m.principal_id === person.id));
    if (open.length) {
      add = `<div class="grpadd"><select id="personCardGroup" aria-label="Floor to add ${esc(name)} to">` +
        open.map(g => `<option value="${esc(g.id)}">${esc(g.name)}</option>`).join("") +
        `</select><button type="button" class="btn primary" id="personCardAdd">Add to floor</button></div>`;
    }
  }
  $("personCardBody").innerHTML =
    `<div class="extsectionhead">Floors</div><div class="grpmembers">${chips}</div>${add}`;
}

async function openPersonCard(name) {
  personCardName = name;
  personCardReturnFocus = document.activeElement;
  $("personCardError").textContent = "";
  const gen = state.buildingGen;
  if (!state.groupsLoaded || !state.profilesList) await fetchGroups(false);
  if (buildingStale(gen)) return;
  renderPersonCard();
  $("personCard").classList.remove("hidden");
  setTimeout(() => $("personCardClose")?.focus(), 0);
}

function closePersonCard() {
  $("personCard").classList.add("hidden");
  personCardName = null;
  if (personCardReturnFocus && document.contains(personCardReturnFocus)) personCardReturnFocus.focus();
  personCardReturnFocus = null;
}

async function personCardClick(event) {
  const person = personCardProfile(personCardName);
  const add = event.target.closest("#personCardAdd");
  const remove = event.target.closest("[data-card-remove]");
  if (!person || (!add && !remove)) return;
  const gen = state.buildingGen;
  const error = add
    ? await addToGroup($("personCardGroup").value, person.id)
    : await removeFromGroup(remove.dataset.cardRemove, person.id);
  if (buildingStale(gen)) return;
  $("personCardError").textContent = error;
  renderPersonCard();
}

$("personCardBody").addEventListener("click", (event) => personCardClick(event));
$("personCardClose").onclick = () => closePersonCard();
$("personCardBackdrop").onclick = () => closePersonCard();
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("personCard").classList.contains("hidden")) closePersonCard();
});

// Titles are not names. A Master or Smaster sets its own display name here;
// the server keeps identity, keys and rank, and refuses titles and names
// someone else already wears. Everyone else's @name is fixed here (the
// server answers 403 to anyone below Smaster), so they are told, not offered.
const PROFILE_TITLE_WORDS = ["master", "smaster", "operator", "lead", "admin", "root"];
function profileRenameHtml(profile, principal) {
  const role = profile.building_role;
  if (role !== "master" && role !== "smaster") {
    return `<div class="profilerename fixed"><span class="plabel">Your @name</span>` +
      `<p class="profilecardhint">@${esc(principal.display_name)} is how people reach you. Only the Master and Smasters rename their own @name here.</p></div>`;
  }
  const isTitle = PROFILE_TITLE_WORDS.includes(String(principal.display_name).toLowerCase());
  const hint = isTitle
    ? `<div class="profilerenamehint">Your name is currently a title. Set your own name so people can tell who you are.</div>`
    : "";
  return `<div class="profilerename">${hint}` +
    `<label for="profileNameInput">Display name (@name)</label>` +
    `<div class="profilerenamerow"><input id="profileNameInput" class="pfield" maxlength="64" autocomplete="off" spellcheck="false" value="${esc(principal.display_name)}" />` +
    `<button type="button" class="pbtn" id="profileRename">Rename</button></div>` +
    `<p class="profilecardhint">A rename keeps your identity, keys and rank. Your past messages follow you to the new name in every loca, and open feeds relabel. A title or a name someone else wears is refused.</p>` +
    `<div class="profilerenamenote" id="profileRenameNote" role="status"></div></div>`;
}

// One radio of the avatar studio, drawn as what it would look like.
function avatarChoiceHtml(group, value, checked, face, label) {
  return `<label class="appearanceopt avopt ${group}opt" title="${esc(label)}"><input type="radio" name="profile-${group}"` +
    ` data-${group === "icon" ? "profile-icon" : `avatar-${group}`} value="${esc(value)}"${checked ? " checked" : ""} />` +
    `<span>${face}<span class="visuallyhidden">${esc(label)}</span></span></label>`;
}
function iconLabel(icon) {
  return icon ? icon[0].toUpperCase() + icon.slice(1) : "No icon";
}

// One's own full name and avatar (PUT /profile/card). Anyone sets their own;
// the @name, and everything it decides, stays as it is. The big avatar above
// previews each pick; Save keeps it.
function profileCardHtml(principal) {
  const card = cardOf(principal) || { full_name: "", icon: "", avatar_color: "", avatar_shape: "" };
  const name = principal.display_name;
  const icon = choice => choice === "initials"
    ? `<span class="avinitials small">${esc(avatarInitials(card.full_name, name))}</span>`
    : profileIconSvg(choice);
  const groups = AVATAR_ICON_GROUPS.map(([title, icons], index) => {
    const list = index === 0 ? ["", ...icons, "initials"] : icons;
    return `<div class="avgroup"><small>${title}</small><div class="appearanceopts profileicons">` +
      list.map(choice => avatarChoiceHtml("icon", choice, (card.icon || "") === choice,
        choice ? icon(choice) : "None", choice === "initials" ? "Initials" : iconLabel(choice))).join("") +
      `</div></div>`;
  }).join("");
  const colors = ["", ...Object.keys(AVATAR_COLORS)].map(color => avatarChoiceHtml("color", color,
    (card.avatar_color || "") === color,
    `<i class="avswatch${color ? "" : " none"}"${color ? ` style="--av-bg:${AVATAR_COLORS[color][0]}"` : ""}></i>`,
    color ? color[0].toUpperCase() + color.slice(1) : "No colour")).join("");
  const shapes = AVATAR_SHAPES.map(shape => avatarChoiceHtml("shape", shape,
    (card.avatar_shape || "circle") === shape, `<i class="avshape av-${shape}"></i>`,
    shape[0].toUpperCase() + shape.slice(1))).join("");
  return `<div class="profilerename profilecard" id="profileCard">` +
    `<label for="profileFullName">Full name</label>` +
    `<div class="profilerenamerow"><input id="profileFullName" class="pfield" maxlength="64" autocomplete="name" spellcheck="false"` +
    ` placeholder="e.g. Ada Lovelace" value="${esc(card.full_name)}" />` +
    `<button type="button" class="pbtn primary" id="profileCardSave">Save</button></div>` +
    `<div class="profilecardhint">Shown beside @${esc(name)}. People still call you @${esc(name)}.</div>` +
    `<div class="avstudio"><span class="plabel" id="profileIconLabel">Avatar</span>` +
      `<div class="avrow" role="radiogroup" aria-label="Avatar colour"><small>Colour</small><div class="appearanceopts">${colors}</div></div>` +
      `<div class="avrow" role="radiogroup" aria-label="Avatar shape"><small>Shape</small><div class="appearanceopts">${shapes}</div></div>` +
      `<div class="avicons" role="radiogroup" aria-label="Profile icon">${groups}</div>` +
    `</div>` +
    `<div class="profilerenamenote" id="profileCardNote" role="status"></div></div>`;
}

// What the studio holds now, saved or not.
function profileCardDraft() {
  const box = $("whoami");
  const picked = selector => box.querySelector(`${selector}:checked`)?.value || "";
  return {
    full_name: $("profileFullName")?.value.trim() || "",
    icon: picked("input[data-profile-icon]"),
    avatar_color: picked("input[data-avatar-color]"),
    avatar_shape: picked("input[data-avatar-shape]"),
  };
}

// The big avatar follows each pick before it is saved.
function previewProfileCard() {
  const preview = $("profileAvatarPreview");
  const principal = state.profile?.principal;
  if (!preview || !principal) return;
  const draft = profileCardDraft();
  preview.innerHTML = avatarHtml(draft, principal.display_name, "xl");
  const saved = cardOf(principal) || {};
  const changed = ["full_name", "icon", "avatar_color"].some(key => (saved[key] || "") !== draft[key])
    || (saved.avatar_shape || "circle") !== (draft.avatar_shape || "circle");
  const note = $("profileCardNote");
  if (note) note.textContent = changed ? "Not saved yet: Save keeps it." : "";
}

async function saveProfileCard() {
  const input = $("profileFullName");
  const note = $("profileCardNote");
  if (!input) return;
  const draft = profileCardDraft();
  const gen = state.buildingGen;
  let response;
  try {
    response = await fetch(`${serverBase()}/profile/card`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ ...draft, full_name: input.value }),
    });
  } catch (e) {
    if (note && !buildingStale(gen)) note.textContent = "could not reach the server — not saved";
    return;
  }
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen) && $("profileCardNote")) $("profileCardNote").textContent = why;
    return;
  }
  const saved = await response.json();
  const principal = state.profile?.principal;
  if (buildingStale(gen) || !principal) return;
  principal.full_name = saved.full_name || undefined;
  principal.icon = saved.icon || undefined;
  principal.avatar_color = saved.avatar_color || undefined;
  principal.avatar_shape = saved.avatar_shape || undefined;
  renderProfile(true);
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const after = $("profileCardNote");
  if (after) after.textContent = "Saved.";
  if (typeof refreshSenderCards === "function") refreshSenderCards();
}

// Set the display name chosen at sign-in. Returns "" on success, or the
// server's reason when it is refused (a title, or a name someone else wears).
async function applyChosenName(name) {
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/profile`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ display_name: name }),
    });
    if (!response.ok) return (await response.text()) || "the name was refused";
    const own = (await response.json()).display_name;
    // The name is its asker's: never worn by whoever signed in meanwhile.
    if (!buildingStale(gen)) adoptOwnName(own);
    return "";
  } catch (e) {
    return "could not reach the server to set the name";
  }
}

function showRenameRefusal(name, reason) {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const note = $("profileRenameNote");
  if (note) note.textContent = `"${name}" was not set: ${reason}`;
  const input = $("profileNameInput");
  if (input) { input.value = name; input.focus(); }
}

async function renameProfile() {
  const input = $("profileNameInput");
  const note = $("profileRenameNote");
  const name = input?.value.trim();
  if (!name || name === state.profile?.principal?.display_name) return;
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile`, {
    method: "PUT",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ display_name: name }),
  });
  if (buildingStale(gen)) return;   // signed out meanwhile: not the next person's name
  if (!response.ok) {
    const why = await response.text();
    if (note && !buildingStale(gen)) note.textContent = why;
    return;
  }
  const renamed = await response.json();
  if (buildingStale(gen)) return;
  const before = state.name;
  adoptOwnName(renamed.display_name);
  // The server relabels history and tells every open feed; reconnecting
  // here covers a loca with nothing said yet (no frame arrives).
  if (before !== renamed.display_name && state.room) openWs();
  await fetchProfile(true);
  if (buildingStale(gen)) return;
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu) menu.open = true;
  const after = $("profileRenameNote");
  if (after) after.textContent = `Renamed to ${renamed.display_name}.`;
}

let profileFetchSequence = 0;
async function fetchProfile(now = false) {
  const box = $("whoami");
  const room = state.room;
  const sequence = ++profileFetchSequence;
  const gen = state.buildingGen;
  try {
    const suffix = room ? `?room=${encodeURIComponent(room)}` : "";
    const [profileResponse, credentialResponse] = await Promise.all([
      fetch(`${serverBase()}/profile${suffix}`, { headers: adminHeaders({}) }),
      fetch(`${serverBase()}/profile/credentials`, { headers: adminHeaders({}) }),
    ]);
    if (!profileResponse.ok) throw new Error("profile unavailable");
    const profile = await profileResponse.json();
    const credentials = credentialResponse.ok ? await credentialResponse.json() : [];
    // Room switches issue overlapping reads. A slow A response must never
    // repaint B's "This Loca" authority after the user has already moved,
    // nor a signed-out person's profile and rank come back after sign-out.
    if (sequence !== profileFetchSequence || room !== state.room || buildingStale(gen)) return;
    state.profile = profile;
    state.credentials = credentials;
    const priorPrincipal = state.principalId;
    state.principalId = profile.principal?.id || null;
    // Identity just hydrated. joinRoom() may have loaded the personal pin under
    // the name fallback before the real principal id arrived; re-key it now (if
    // still in a room and the principal actually changed) so a pin stored under
    // the principal key reappears after a slow /profile.
    if (state.room && state.principalId && state.principalId !== priorPrincipal) {
      loadPinned();
    }
    if (state.profile.principal?.display_name) {
      state.name = state.profile.principal.display_name;
      $("name").value = state.name;
    }
    if (state.principalId && state.principalId !== state.appearancePrincipal) {
      fetchAppearance();
    }
    markLocaContextReady("profile", room);
    loadRoomPreferences();
    renderRooms();
    renderProfile(now);
    renderLocaSidebar();
    renderOperatorControls();
    // What a seat shows can depend on this person's role in the loca (an
    // Operator sees an agent's kit version): draw the roster again with it.
    if (state.room) renderMembers();
  } catch (error) {
    if (sequence !== profileFetchSequence || room !== state.room || buildingStale(gen)) return;
    state.profile = null;
    state.credentials = [];
    if (profileRedrawMustWait(box, now)) {
      markLocaContextReady("profile", room);
      renderLocaSidebar();
      return;
    }
    const kept = accountMenuState(box);
    box.innerHTML = profileFallbackHtml("Profile is unavailable.");
    restoreAccountMenu(box, kept);
    box.classList.add("on");
    markLocaContextReady("profile", room);
    renderLocaSidebar();
  }
}

async function createProfileCredential() {
  const input = $("credentialLabel");
  const label = input?.value.trim();
  if (!label) return;
  // The key made here is its asker's: its secret is shown to nobody who
  // signed in while the request, its body or the profile refresh was out.
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile/credentials`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ label }),
  });
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen)) addSys(`could not create access key: ${why}`);
    return;
  }
  const created = await response.json();
  if (buildingStale(gen)) return;
  await fetchProfile(true);
  if (buildingStale(gen)) return;
  $("whoami").querySelector("details.profilemenu").open = true;
  $("whoami").querySelector("details.profileaccess").open = true;
  const secret = $("credentialSecret");
  secret.classList.remove("hidden");
  secret.innerHTML = `<b>Copy now — shown once</b><code>${esc(created.secret)}</code>` +
    `<button type="button" id="credentialCopy">Copy</button>`;
  $("credentialCopy").onclick = async () => {
    const ok = await copyText(created.secret, secret.querySelector("code"));
    $("credentialCopy").textContent = ok ? "Copied" : "Selected — press Ctrl+C";
  };
}

async function revokeProfileCredential(id, current) {
  const warning = current
    ? "Revoke the key behind this session? You will be signed out now."
    : "Revoke this access key? Other keys and the profile stay active.";
  if (!confirm(warning)) return;
  // Revoking one's own key ends one's own session. Answered after someone
  // else signed in, it must not end theirs.
  const gen = state.buildingGen;
  const response = await fetch(`${serverBase()}/profile/credentials/${encodeURIComponent(id)}`, {
    method: "DELETE", headers: adminHeaders({}),
  });
  if (buildingStale(gen)) return;
  if (!response.ok) {
    const why = await response.text();
    if (!buildingStale(gen)) addSys(`could not revoke access key: ${why}`);
    return;
  }
  if (current) {
    try {
      localStorage.removeItem("loca-admin-session");
      sessionStorage.removeItem("loca-admin-session");
    } catch (error) {}
    state.session = null;
    state.adminSession = false;
    state.sessionExpires = null;
    resetRoomPreferenceIdentity();
    state.profile = null;
    setLocked(true);
    clearBuildingData();
    clearPersonWords();
    setConnOpen(true);
    return;
  }
  await fetchProfile(true);
}

$("whoami").addEventListener("keydown", event => {
  if (event.key === "Enter" && event.target.id === "profileNameInput") renameProfile();
  if (event.key === "Enter" && event.target.id === "profileFullName") saveProfileCard();
  if (event.key === "Escape") closeAccountMenu();
});

function closeAccountMenu() {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (!menu?.open) return;
  menu.open = false;
  menu.querySelector("summary")?.focus();
}

async function copyServerAddress(button) {
  const ok = await copyText($("accountServer")?.textContent || "", $("accountServer"));
  button.textContent = ok ? "Copied" : "Selected — press Ctrl+C";
}

$("whoami").addEventListener("click", event => {
  const create = event.target.closest("#credentialCreate");
  if (create) { createProfileCredential(); return; }
  if (event.target.closest("#profileRename")) { renameProfile(); return; }
  if (event.target.closest("#profileCardSave")) { saveProfileCard(); return; }
  if (event.target.closest("#profileClose")) { closeAccountMenu(); return; }
  if (event.target.closest("#appearanceReset")) { resetAppearance(); return; }
  const copy = event.target.closest("#accountServerCopy");
  if (copy) { copyServerAddress(copy); return; }
  const jump = event.target.closest("[data-profile-jump]");
  if (jump) { $(jump.dataset.profileJump)?.scrollIntoView({ block: "start" }); return; }
  const revoke = event.target.closest("[data-revoke-credential]");
  if (revoke) revokeProfileCredential(
    revoke.dataset.revokeCredential,
    revoke.dataset.current === "1",
  );
});

$("whoami").addEventListener("change", event => {
  const input = event.target.closest("input[data-appearance]");
  if (input?.checked) {
    // "Custom colour" with no colour picked yet takes the picker's colour.
    if (input.value === "custom" && !currentAppearance().accent_custom) {
      saveAppearancePatch({ accent: "custom", accent_custom: $("accentCustomInput")?.value || "#2ee6c8" });
    } else {
      saveAppearance(input.dataset.appearance, input.value);
    }
  }
  if (event.target.id === "accentCustomInput") {
    saveAppearancePatch({ accent: "custom", accent_custom: event.target.value.toLowerCase() });
    syncAppearanceInputs();
  }
  if (event.target.matches?.("input[data-profile-icon], input[data-avatar-color], input[data-avatar-shape]")) previewProfileCard();
});

// The picker previews while it is dragged; the change above saves.
$("whoami").addEventListener("input", event => {
  if (event.target.id === "accentCustomInput") {
    applyAppearance({ ...currentAppearance(), accent: "custom", accent_custom: event.target.value.toLowerCase() });
  }
  if (event.target.id === "profileFullName") previewProfileCard();
});

document.addEventListener("click", event => {
  const menu = $("whoami").querySelector("details.profilemenu");
  if (menu?.open && !$("whoami").contains(event.target)) menu.open = false;
});
