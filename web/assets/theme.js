"use strict";
// Personal appearance, applied before first paint. This file loads in <head>
// (CSP forbids inline scripts) and only touches <html>, so the cached look is
// on screen before the body renders. profile.js later reconciles with the
// server copy (GET /profile/preferences) once the session resolves.

const APPEARANCE_CHOICES = {
  theme: ["dark", "ember", "contrast", "night", "light", "paper"],
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
  theme: "dark", accent: "teal", density: "comfortable", font_size: "normal", font_family: "mono",
  accent_custom: "", layout: "bubbles", time_format: "auto", measure: "normal", own_tint: "on",
  motion: "system",
};
// Themes drawn on a light page; the rest are dark.
const LIGHT_THEMES = ["light", "paper"];
// Each theme's page and raised surface, for keeping a custom accent readable
// (the same values as the theme's --bg and --panel2 tokens).
const THEME_SURFACES = {
  dark: ["#0b0e12", "#161b23"], ember: ["#16110e", "#231b16"], contrast: ["#000000", "#121212"],
  night: ["#121522", "#1b2032"], light: ["#f5f7fa", "#e9edf2"], paper: ["#f6f1e7", "#ebe3d3"],
};
const CUSTOM_ACCENT = /^#[0-9a-f]{6}$/;
const APPEARANCE_CACHE_KEY = "loca-appearance";

function normalizeAppearance(value) {
  const next = { ...APPEARANCE_DEFAULTS };
  if (value && typeof value === "object") {
    for (const [key, allowed] of Object.entries(APPEARANCE_CHOICES)) {
      if (allowed.includes(value[key])) next[key] = value[key];
    }
    const custom = String(value.accent_custom || "").toLowerCase();
    if (CUSTOM_ACCENT.test(custom)) next.accent_custom = custom;
  }
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

function applyAppearance(value) {
  const prefs = normalizeAppearance(value);
  const root = document.documentElement;
  root.dataset.theme = prefs.theme;
  root.dataset.accent = prefs.accent;
  root.dataset.density = prefs.density;
  root.dataset.fontSize = prefs.font_size;
  root.dataset.fontFamily = prefs.font_family;
  root.dataset.layout = prefs.layout;
  root.dataset.timeFormat = prefs.time_format;
  root.dataset.measure = prefs.measure;
  root.dataset.ownTint = prefs.own_tint;
  root.dataset.motion = prefs.motion;
  root.dataset.scheme = LIGHT_THEMES.includes(prefs.theme) ? "light" : "dark";
  // A custom accent is set on <html> itself; any other accent clears it, so
  // the theme's own accent rules apply again. Custom with no colour saved
  // reads as teal.
  const custom = prefs.accent === "custom" && prefs.accent_custom ? readableAccent(prefs.accent_custom, prefs.theme) : null;
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
    return normalizeAppearance(JSON.parse(localStorage.getItem(APPEARANCE_CACHE_KEY) || "null"));
  } catch (error) {
    return { ...APPEARANCE_DEFAULTS };
  }
}

applyAppearance(cachedAppearance());
