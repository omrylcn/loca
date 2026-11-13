"use strict";
// Personal appearance, applied before first paint. This file loads in <head>
// (CSP forbids inline scripts) and only touches <html>, so the cached look is
// on screen before the body renders. profile.js later reconciles with the
// server copy (GET /profile/preferences) once the session resolves.

const APPEARANCE_CHOICES = {
  theme: ["dark", "light"],
  accent: ["teal", "blue", "violet", "amber", "rose"],
  density: ["comfortable", "compact"],
  font_size: ["normal", "large"],
};
const APPEARANCE_DEFAULTS = { theme: "dark", accent: "teal", density: "comfortable", font_size: "normal" };
const APPEARANCE_CACHE_KEY = "loca-appearance";

function normalizeAppearance(value) {
  const next = { ...APPEARANCE_DEFAULTS };
  if (value && typeof value === "object") {
    for (const [key, allowed] of Object.entries(APPEARANCE_CHOICES)) {
      if (allowed.includes(value[key])) next[key] = value[key];
    }
  }
  return next;
}

function applyAppearance(value) {
  const prefs = normalizeAppearance(value);
  const root = document.documentElement;
  root.dataset.theme = prefs.theme;
  root.dataset.accent = prefs.accent;
  root.dataset.density = prefs.density;
  root.dataset.fontSize = prefs.font_size;
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
