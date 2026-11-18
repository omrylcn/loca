"use strict";
// Sounds: four short chimes for what arrives, each person's own choice in
// Profile → Sounds (saved in GET/PUT /profile/preferences beside the look,
// cached on this device the way theme.js caches the look).
//
// Every sound is made here with the Web Audio API: a few sine oscillators
// under a soft envelope, none longer than 300 ms. No audio file, no request.
// The AudioContext is made only after this page has had a click or a key
// press (browsers refuse sound before one) and every failure is silent.
//
// Where the arrivals come from (one-line hooks):
//   socket.js  a live message in the loca on screen; a reminder frame
//   state.js   the unread count's tail of every other loca and conversation
//   notifications.js  a new Building notification (the bell)
// History and the first unread count after the page loads are what was
// already there, so they never sound. Nothing is ever played for one's own
// message, and a system line ("loca") only when it is addressed to you.

const SOUND_CHOICES = {
  sound: ["on", "off"],
  sound_message: ["off", "on"],
  sound_mention: ["on", "off"],
  sound_dm: ["on", "off"],
  sound_notice: ["on", "off"],
  sound_volume: ["medium", "low", "high"],
  sound_background: ["on", "off"],
};
// Each list starts with its default, as on the server (preferences/sounds.rs).
const SOUND_DEFAULTS = Object.fromEntries(Object.entries(SOUND_CHOICES).map(([key, values]) => [key, values[0]]));
const SOUND_CACHE_KEY = "loca-sounds";
const SOUND_THROTTLE_MS = 1500;
// The loudest point of a sound, by volume. Soft on purpose.
const SOUND_VOLUME_GAIN = { low: 0.07, medium: 0.14, high: 0.26 };
const SOUND_REMINDER_REASONS = ["goal_reminder", "task_reminder", "wait_overdue", "wait_cycle", "room_silence"];
// The kinds, in the order the Sounds section lists them.
const SOUND_KINDS = [
  { kind: "message", key: "sound_message", label: "Messages", hint: "Anything new in a loca you are in.", noun: "message" },
  { kind: "mention", key: "sound_mention", label: "Mentions & replies to me", hint: "Also a reminder addressed to you.", noun: "mention" },
  { kind: "dm", key: "sound_dm", label: "Private messages", hint: "", noun: "private message" },
  { kind: "notice", key: "sound_notice", label: "Notifications", hint: "The Building's bell.", noun: "notification" },
];

// ---- The four sounds -------------------------------------------------------
// Each is a list of tones: start (s), frequency (Hz), an optional glide to
// `to` Hz over `glide` s, its share of the volume, attack and decay (s).
// Attack rises exponentially from silence, decay falls back to it.
const SOUND_SHAPES = {
  // (a) A message: one soft G5 tick.
  message: [
    { at: 0, freq: 784, peak: 0.55, attack: 0.005, decay: 0.12 },
  ],
  // (b) A mention: a rising fifth, E5 then B5, each with a faint octave.
  mention: [
    { at: 0, freq: 659.25, peak: 0.8, attack: 0.006, decay: 0.16 },
    { at: 0, freq: 1318.5, peak: 0.12, attack: 0.006, decay: 0.09 },
    { at: 0.09, freq: 987.77, peak: 0.8, attack: 0.006, decay: 0.19 },
    { at: 0.09, freq: 1975.5, peak: 0.12, attack: 0.006, decay: 0.1 },
  ],
  // (c) A private message: two small upward bubbles.
  dm: [
    { at: 0, freq: 520, to: 780, glide: 0.06, peak: 0.7, attack: 0.006, decay: 0.11 },
    { at: 0.12, freq: 620, to: 930, glide: 0.06, peak: 0.7, attack: 0.006, decay: 0.13 },
  ],
  // (d) The bell: C6 with G6 and a faint inharmonic overtone, struck once.
  notice: [
    { at: 0, freq: 1046.5, peak: 0.7, attack: 0.003, decay: 0.28 },
    { at: 0, freq: 1568, peak: 0.25, attack: 0.003, decay: 0.2 },
    { at: 0, freq: 2888, peak: 0.06, attack: 0.003, decay: 0.12 },
  ],
};

// How long a sound lasts, in seconds (each is at most 0.3).
function soundLength(kind) {
  return Math.max(...SOUND_SHAPES[kind].map(tone => tone.at + tone.attack + tone.decay));
}

// ---- Preferences -----------------------------------------------------------
function normalizeSounds(value) {
  const next = { ...SOUND_DEFAULTS };
  if (value && typeof value === "object") {
    for (const [key, allowed] of Object.entries(SOUND_CHOICES)) {
      if (allowed.includes(value[key])) next[key] = value[key];
    }
  }
  return next;
}

function cachedSounds() {
  try {
    return normalizeSounds(JSON.parse(localStorage.getItem(SOUND_CACHE_KEY) || "null"));
  } catch (error) {
    return { ...SOUND_DEFAULTS };
  }
}

function cacheSounds(prefs) {
  try { localStorage.setItem(SOUND_CACHE_KEY, JSON.stringify(normalizeSounds(prefs))); } catch (error) {}
}

let soundPrefs = cachedSounds();

function applySounds(value) {
  soundPrefs = normalizeSounds(value);
  cacheSounds(soundPrefs);
  syncSoundInputs();
}

// profile.js fetchAppearance: the profile's copy, read after sign-in. The
// read is skipped there when a preference (a sound included) was changed
// while it was in flight.
function soundsFromServer(prefs) {
  applySounds(prefs);
}

let soundSaveSequence = 0;
// Apply at once, keep on this device, then save to the profile.
async function saveSoundPatch(patch) {
  // A sound is part of the same preferences document as the look: a read of
  // it that is in flight now is older than this change (profile.js).
  if (typeof appearanceEdits === "number") appearanceEdits++;
  applySounds({ ...soundPrefs, ...patch });
  const sequence = ++soundSaveSequence;
  const gen = state.buildingGen;
  try {
    const response = await fetch(`${serverBase()}/profile/preferences`, {
      method: "PUT",
      headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(patch),
    });
    if (!response.ok) throw new Error(await response.text());
    const saved = await response.json();
    if (sequence !== soundSaveSequence || buildingStale(gen)) return;
    applySounds(saved);
    setSoundStatus("");
  } catch (error) {
    if (sequence === soundSaveSequence && !buildingStale(gen)) setSoundStatus("Not saved to your profile; kept on this device.");
  }
}

// ---- Playing ---------------------------------------------------------------
let soundContext = null;
let soundGesture = false;
const soundLastAt = {};

// A click or a key press anywhere lets the page make sound from now on.
function soundUnlock() {
  soundGesture = true;
  try { if (soundContext && soundContext.state === "suspended") soundContext.resume().catch(() => {}); } catch (error) {}
}
for (const type of ["pointerdown", "keydown"]) window.addEventListener(type, soundUnlock, { capture: true, passive: true });

function soundAudio() {
  if (!soundGesture) return null;
  try {
    if (!soundContext) {
      const Context = window.AudioContext || window.webkitAudioContext;
      if (!Context) return null;
      soundContext = new Context();
    }
    if (soundContext.state === "suspended") soundContext.resume().catch(() => {});
    return soundContext;
  } catch (error) {
    return null;
  }
}

function soundSynth(context, kind, volume) {
  const out = context.createGain();
  out.gain.value = SOUND_VOLUME_GAIN[volume] || SOUND_VOLUME_GAIN.medium;
  out.connect(context.destination);
  const start = context.currentTime + 0.01;
  for (const tone of SOUND_SHAPES[kind]) {
    const at = start + tone.at;
    const end = at + tone.attack + tone.decay;
    const osc = context.createOscillator();
    const env = context.createGain();
    osc.type = "sine";
    osc.frequency.setValueAtTime(tone.freq, at);
    if (tone.to) osc.frequency.exponentialRampToValueAtTime(tone.to, at + tone.glide);
    env.gain.setValueAtTime(0.0001, at);
    env.gain.exponentialRampToValueAtTime(tone.peak, at + tone.attack);
    env.gain.exponentialRampToValueAtTime(0.0001, end);
    osc.connect(env).connect(out);
    osc.start(at);
    osc.stop(end + 0.02);
  }
  setTimeout(() => { try { out.disconnect(); } catch (error) {} }, 600);
}

// Play one kind. A preview (the Play button) is the person asking, so it is
// neither throttled nor held to the switches. Tests read window.__soundLog
// when they put one there; nothing is logged otherwise.
function playSound(kind, preview = false) {
  if (!SOUND_SHAPES[kind]) return false;
  const now = Date.now();
  if (!preview) {
    if (now - (soundLastAt[kind] || 0) < SOUND_THROTTLE_MS) return false;
    soundLastAt[kind] = now;
  }
  if (Array.isArray(window.__soundLog)) window.__soundLog.push({ kind, preview, volume: soundPrefs.sound_volume, at: now });
  const context = soundAudio();
  if (!context) return true;
  try { soundSynth(context, kind, soundPrefs.sound_volume); } catch (error) {}
  return true;
}

function soundWanted(kind) {
  const entry = SOUND_KINDS.find(k => k.kind === kind);
  if (!entry || soundPrefs.sound !== "on" || soundPrefs[entry.key] !== "on") return false;
  return soundPrefs.sound_background !== "on" || document.hidden === true;
}

// A message, a reminder or a notification is heard once, however often its
// frame or its unread tail comes back.
const soundHeard = new Set();
function soundFirstHearing(room, id) {
  if (id === undefined || id === null) return true;
  const key = `${room}\u0000${id}`;
  if (soundHeard.has(key)) return false;
  if (soundHeard.size > 4000) soundHeard.clear();
  soundHeard.add(key);
  return true;
}

// Which sound a message calls for in `room`, or null.
function soundKindOfMessage(message, room) {
  const me = state.name;
  if (!message || !me || message.sender === me) return null;
  const toMe = message.target === me || message.reply_to_sender === me || isMentionOfMe(message, me);
  // A system line sounds only when it is addressed to you.
  if (message.sender === "loca") return toMe ? "mention" : null;
  if (isDm(room)) return "dm";
  return toMe ? "mention" : "message";
}

// A loca one is in or watching: the one on screen, a conversation, every loca
// a member is listed (its seats), and for a Master or Smaster, who may open
// every loca, only those they sat down in (state.js isWatchingDoor). No
// per-loca mute exists; Hide is navigation, not silence.
function soundLocaIsMine(room) {
  if (room === state.room || isDm(room)) return true;
  if (state.adminSession !== true) return true;
  return isRankHome(room) ? !roomSet(DOOR_ROOMS_KEY).has(room) : roomSet(SEATED_ROOMS_KEY).has(room);
}

// The first and only the first wanted kind of a batch: one arrival, one chime.
function soundForKinds(kinds) {
  const kind = ["dm", "mention", "message"].find(k => kinds.has(k) && soundWanted(k));
  if (kind) playSound(kind);
}

// socket.js: a live message in the loca on screen (never history).
function soundForLiveMessage(message) {
  if (!message || !soundFirstHearing(state.room, message.id)) return;
  const kind = soundKindOfMessage(message, state.room);
  if (kind) soundForKinds(new Set([kind]));
}

// Messages older than this page are history, not arrivals.
const SOUND_PAGE_START = Date.now();
// state.js updateUnreadCounts: the unread tail of `room`, with the last id
// counted before it. Undefined means the first count since this page (or
// this person) started: then only what was said since the page opened is new.
function soundForTail(room, messages, countedBefore) {
  if (!Array.isArray(messages) || !soundLocaIsMine(room)) return;
  const kinds = new Set();
  for (const message of messages) {
    const fresh = countedBefore === undefined
      ? Number(message.ts || 0) >= SOUND_PAGE_START
      : Number(message.id || 0) > Number(countedBefore || 0);
    if (!fresh || !soundFirstHearing(room, message.id)) continue;
    const kind = soundKindOfMessage(message, room);
    if (kind) kinds.add(kind);
  }
  soundForKinds(kinds);
}

// socket.js: an attention frame. A reminder addressed to you sounds like a
// mention, once per delivery attempt; anyone else's stays quiet.
function soundForAttention(attention) {
  if (!attention || !SOUND_REMINDER_REASONS.includes(attention.reason)) return;
  if (attention.status && attention.status !== "open") return;
  if (attention.room && attention.room !== state.room) return;
  const me = state.name;
  const audience = attention.audience || {};
  const toMe = attention.owner ? attention.owner === me
    : audience.kind === "person" ? audience.name === me
    : audience.kind === "group" ? (audience.names || []).includes(me)
    : !!me && state.settings?.lead === me;
  if (!toMe || !soundFirstHearing(`attention:${attention.room || state.room}`, `${attention.id}:${attention.attempt || 1}`)) return;
  if (soundWanted("mention")) playSound("mention");
}

// notifications.js: new, unread notifications (never the bell's first read).
function soundForNotices(notices) {
  const fresh = (notices || []).filter(n => !n.read && n.from !== state.name && soundFirstHearing("notice", n.id));
  if (fresh.length && soundWanted("notice")) playSound("notice");
}

// ---- Profile → Sounds ------------------------------------------------------
const SOUND_PLAY_ICON = `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" focusable="false">` +
  `<path d="M5 3.5v9l7-4.5z" fill="currentColor"/></svg>`;

function soundSwitchHtml(key, label, hint, play) {
  const id = `soundSwitch-${key}`;
  const hintId = hint ? `soundHint-${key}` : "";
  const checked = soundPrefs[key] === "on" ? " checked" : "";
  return `<div class="soundrow${play ? " haskind" : ""}">` +
    `<input type="checkbox" role="switch" class="soundswitch" id="${id}" data-sound="${key}"${checked}` +
      (hintId ? ` aria-describedby="${hintId}"` : "") + ` />` +
    `<span class="soundtext"><label for="${id}">${esc(label)}</label>` +
      (hint ? `<small id="${hintId}">${esc(hint)}</small>` : "") + `</span>` +
    (play ? `<button type="button" class="pbtn small ghost soundplay" data-sound-play="${play.kind}"` +
      ` aria-label="Play the ${play.noun} sound">${SOUND_PLAY_ICON}<span>Play</span></button>` : "") +
    `</div>`;
}

function soundVolumeHtml() {
  const names = { low: "Low", medium: "Medium", high: "High" };
  const options = ["low", "medium", "high"].map(value =>
    `<label class="appearanceopt"><input type="radio" name="sound-volume" data-sound="sound_volume" value="${value}"` +
    `${soundPrefs.sound_volume === value ? " checked" : ""} /><span>${names[value]}</span></label>`).join("");
  return `<div class="appearancerow soundvolume" role="radiogroup" aria-labelledby="soundVolumeLabel">` +
    `<span id="soundVolumeLabel">Volume</span><div class="appearanceopts">${options}</div></div>`;
}

// profile.js renderProfile: the section, after the look.
function soundsSectionHtml() {
  return `<section class="psec soundsec${soundPrefs.sound === "on" ? "" : " soundsoff"}" id="profileSecSounds" aria-labelledby="profileSecSounds-h">` +
    `<h3 class="psechead" id="profileSecSounds-h">Sounds</h3>` +
    soundSwitchHtml("sound", "Sounds", "Short chimes made in the browser, for you only.", null) +
    `<div class="soundkinds" role="group" aria-label="Play a sound for">` +
      SOUND_KINDS.map(k => soundSwitchHtml(k.key, k.label, k.hint, k)).join("") + `</div>` +
    soundVolumeHtml() +
    soundSwitchHtml("sound_background", "Only when Loca is in the background", "Quiet while Loca is the window in front.", null) +
    `<div class="appearancestatus soundstatus" id="soundStatus" role="status"></div></section>`;
}

function syncSoundInputs() {
  document.querySelectorAll("input[data-sound]").forEach(input => {
    input.checked = input.type === "radio"
      ? soundPrefs[input.dataset.sound] === input.value
      : soundPrefs[input.dataset.sound] === "on";
  });
  $("profileSecSounds")?.classList.toggle("soundsoff", soundPrefs.sound !== "on");
}

function setSoundStatus(text) {
  const status = $("soundStatus");
  if (status) status.textContent = text;
}

$("whoami").addEventListener("change", event => {
  const input = event.target.closest?.("input[data-sound]");
  if (!input) return;
  if (input.type === "radio" && !input.checked) return;
  saveSoundPatch({ [input.dataset.sound]: input.type === "radio" ? input.value : (input.checked ? "on" : "off") });
});

$("whoami").addEventListener("click", event => {
  const play = event.target.closest?.("[data-sound-play]");
  if (!play) return;
  soundGesture = true;
  playSound(play.dataset.soundPlay, true);
});
