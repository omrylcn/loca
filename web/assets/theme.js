"use strict";
// Personal appearance, applied before first paint. This file loads in <head>
// (CSP forbids inline scripts) and only touches <html>, so the cached look is
// on screen before the body renders. profile.js later reconciles with the
// server copy (GET /profile/preferences) once the session resolves.

const APPEARANCE_CHOICES = {
  theme: ["loca", "calm", "console", "dark", "ember", "contrast", "night", "light", "paper"],
  accent: ["teal", "blue", "indigo", "violet", "rose", "orange", "amber", "green", "sky", "custom"],
  density: ["compact", "comfortable", "spacious"],
  font_size: ["small", "normal", "large", "larger"],
  font_family: ["mono", "sans", "serif"],
  layout: ["bubbles", "flat"],
  time_format: ["auto", "h24", "h12", "relative"],
  measure: ["narrow", "normal", "wide"],
  own_tint: ["on", "off"],
  motion: ["system", "reduced"],
};
const APPEARANCE_DEFAULTS = {
  theme: "loca", accent: "teal", density: "comfortable", font_size: "normal", font_family: "sans",
  accent_custom: "", layout: "bubbles", time_format: "auto", measure: "normal", own_tint: "on",
  motion: "system",
};
// Themes drawn on a light page; the rest are dark.
const LIGHT_THEMES = ["loca", "calm", "light", "paper"];
// Each theme's page and raised surface, for keeping a custom accent readable
// (the same values as the theme's --bg and --panel2 tokens).
const THEME_SURFACES = {
  loca: ["#f6f1e7", "#ece4d5"], calm: ["#f7f8f6", "#eef1ee"], console: ["#0e1215", "#1b2228"],
  dark: ["#0b0e12", "#161b23"], ember: ["#16110e", "#231b16"], contrast: ["#000000", "#121212"],
  night: ["#121522", "#1b2032"], light: ["#f5f7fa", "#e9edf2"], paper: ["#f6f1e7", "#ebe3d3"],
};
// Loca has three themes: Loca, Calm and Console. A look saved with one of
// the older themes reads as its successor — a dark one as Console, Light as
// Calm, Paper as Loca — so everyone sees the redesign. The server still
// accepts and keeps the older names; only the page draws the three.
const THEME_SUCCESSOR = {
  dark: "console", ember: "console", contrast: "console", night: "console",
  light: "calm", paper: "loca",
};
const PICKED_THEMES = ["loca", "calm", "console"];
const CUSTOM_ACCENT = /^#[0-9a-f]{6}$/;
const APPEARANCE_CACHE_KEY = "loca-appearance";

// A look cached by an older page keeps the theme it had: one that names none
// meant the dark theme of that time (read as Console), as the server reads a
// saved row. A face it does not name is today's, sans.
const SAVED_LOOK_BASELINE = { ...APPEARANCE_DEFAULTS, theme: "dark" };

function normalizeAppearance(value, base = APPEARANCE_DEFAULTS) {
  const next = { ...base };
  if (value && typeof value === "object") {
    for (const [key, allowed] of Object.entries(APPEARANCE_CHOICES)) {
      if (allowed.includes(value[key])) next[key] = value[key];
    }
    const custom = String(value.accent_custom || "").toLowerCase();
    if (CUSTOM_ACCENT.test(custom)) next.accent_custom = custom;
  }
  if (THEME_SUCCESSOR[next.theme]) next.theme = THEME_SUCCESSOR[next.theme];
  return next;
}

// ---- A custom accent, kept readable -------------------------------------
function hexRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function rgbHex(rgb) {
  return "#" + rgb.map(v => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("");
}
function luminance(hex) {
  const [r, g, b] = hexRgb(hex).map(v => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrastRatio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
function mixHex(a, b, t) {
  const x = hexRgb(a), y = hexRgb(b);
  return rgbHex(x.map((v, i) => v + (y[i] - v) * t));
}
// The accent is text on the page and on raised surfaces, so it must keep
// 4.5:1 on both. A colour that does not is moved toward white (dark
// themes) or black (light themes) in small steps until it does; the step
// count says how far it had to move. The ink on an accent fill is whichever
// of the theme's page colour or white/black reads better on it.
function readableAccent(hex, theme) {
  const [bg, raised] = THEME_SURFACES[theme] || THEME_SURFACES.dark;
  const toward = LIGHT_THEMES.includes(theme) ? "#000000" : "#ffffff";
  let accent = hex;
  let steps = 0;
  while (steps < 20 && (contrastRatio(accent, bg) < 4.5 || contrastRatio(accent, raised) < 4.5)) {
    steps++;
    accent = mixHex(hex, toward, steps * 0.05);
  }
  const strong = mixHex(accent, toward, 0.22);
  const ink = contrastRatio("#ffffff", accent) >= contrastRatio("#0b0e12", accent) ? "#ffffff" : "#0b0e12";
  return { accent, strong, ink, adjusted: steps > 0 };
}

// "Follow my system": Calm while the device is in its light scheme, Console
// in its dark one, switching live. The server keeps theme names only
// (store/preferences.rs), so this is a setting of this browser, kept beside
// the cached look; the saved theme is left as it is and returns when it is
// turned off.
const FOLLOW_SYSTEM_KEY = "loca-follow-system";
const SYSTEM_DARK = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

function followsSystem() {
  try { return localStorage.getItem(FOLLOW_SYSTEM_KEY) === "on"; } catch (error) { return false; }
}

function setFollowSystem(on) {
  try {
    if (on) localStorage.setItem(FOLLOW_SYSTEM_KEY, "on");
    else localStorage.removeItem(FOLLOW_SYSTEM_KEY);
  } catch (error) {}
}

function systemTheme() {
  return SYSTEM_DARK && SYSTEM_DARK.matches ? "console" : "calm";
}

// The theme on screen: the device's while following it, else the saved one.
function effectiveTheme(prefs) {
  return followsSystem() ? systemTheme() : prefs.theme;
}

let lastAppliedAppearance = null;

function applyAppearance(value) {
  const prefs = normalizeAppearance(value);
  lastAppliedAppearance = prefs;
  const theme = effectiveTheme(prefs);
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.dataset.accent = prefs.accent;
  root.dataset.density = prefs.density;
  root.dataset.fontSize = prefs.font_size;
  root.dataset.fontFamily = prefs.font_family;
  root.dataset.layout = prefs.layout;
  root.dataset.timeFormat = prefs.time_format;
  root.dataset.measure = prefs.measure;
  root.dataset.ownTint = prefs.own_tint;
  root.dataset.motion = prefs.motion;
  root.dataset.scheme = LIGHT_THEMES.includes(theme) ? "light" : "dark";
  // A custom accent is set on <html> itself; any other accent clears it, so
  // the theme's own accent rules apply again. Custom with no colour saved
  // reads as teal.
  const custom = prefs.accent === "custom" && prefs.accent_custom ? readableAccent(prefs.accent_custom, theme) : null;
  if (prefs.accent === "custom" && !custom) root.dataset.accent = "teal";
  for (const [name, value] of [["--accent", custom?.accent], ["--accent2", custom?.strong], ["--on-accent", custom?.ink]]) {
    if (value) root.style.setProperty(name, value);
    else root.style.removeProperty(name);
  }
  return prefs;
}

function cacheAppearance(prefs) {
  try {
    localStorage.setItem(APPEARANCE_CACHE_KEY, JSON.stringify(normalizeAppearance(prefs)));
  } catch (error) {}
}

function cachedAppearance() {
  try {
    const cached = JSON.parse(localStorage.getItem(APPEARANCE_CACHE_KEY) || "null");
    return normalizeAppearance(cached, cached ? SAVED_LOOK_BASELINE : APPEARANCE_DEFAULTS);
  } catch (error) {
    return { ...APPEARANCE_DEFAULTS };
  }
}

applyAppearance(cachedAppearance());

// The device changing scheme, or another tab turning following on or off,
// redraws the look already applied (profile.js's sample and inputs too).
function reapplyFollowedTheme() {
  applyAppearance(lastAppliedAppearance || cachedAppearance());
  if (typeof syncAppearanceInputs === "function") syncAppearanceInputs();
}
if (SYSTEM_DARK) {
  const onSchemeChange = () => { if (followsSystem()) reapplyFollowedTheme(); };
  if (typeof SYSTEM_DARK.addEventListener === "function") SYSTEM_DARK.addEventListener("change", onSchemeChange);
  else if (typeof SYSTEM_DARK.addListener === "function") SYSTEM_DARK.addListener(onSchemeChange);
}
window.addEventListener("storage", event => {
  if (event.key === FOLLOW_SYSTEM_KEY) reapplyFollowedTheme();
});
