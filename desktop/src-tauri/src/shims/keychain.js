
(function () {
  try {
    // This window shows one origin, the app's own (main.rs APP_ORIGIN), which
    // the shell names next to the boot values. The init script runs on every
    // top-level page, so on any other origin nothing is handed over and
    // nothing is installed: no cache, no storage override, no window.name.
    var origins = window.__LOCA_KC_ORIGINS__;
    try { delete window.__LOCA_KC_ORIGINS__; } catch (e) {}
    if (!Array.isArray(origins) || origins.indexOf(location.origin) === -1) {
      try { delete window.__LOCA_KC_BOOT__; } catch (e) {}
      return;
    }
    var KC_KEYS = ["loca-seat", "loca-admin-session"];
    var boot = window.__LOCA_KC_BOOT__ || {};
    var cache = {};
    KC_KEYS.forEach(function (k) { if (boot[k] != null) cache[k] = String(boot[k]); });
    try { delete window.__LOCA_KC_BOOT__; } catch (e) {}

    function invoke() {
      var t = window.__TAURI__;
      var fn = (t && t.core && t.core.invoke) || (t && t.invoke);
      return fn ? fn : null;
    }
    // Only localStorage holds the credentials. The override sits on
    // Storage.prototype, which sessionStorage shares; the web UI clears a
    // legacy sessionStorage copy right after saving the localStorage one
    // (state.js), and that must not delete the credential it just saved.
    function isKc(store, k) { return store === window.localStorage && KC_KEYS.indexOf(k) !== -1; }

    var proto = window.Storage && window.Storage.prototype;
    if (!proto) return; // no Web Storage -> nothing to guard
    var rawGet = proto.getItem, rawSet = proto.setItem, rawRemove = proto.removeItem;

    // The boot values are the shell's one-time hand-over to this window: what
    // it read from the keychain when the window was built. The init script
    // brings back exactly those on every load of the page, so only the first
    // load may take them. After that the window's current values are the ones
    // its page last stored, kept in its own sessionStorage (in memory, this
    // window only, gone with it): the web UI reloads itself when its server
    // restarts and must come back as whoever is signed in now, not as whoever
    // was at launch.
    //
    // Two records say which load this is, and each checks the other: a
    // counter in window.name (it survives a reload of this window, and
    // assigning it cannot fail on a full or refused storage) and the kept
    // copy, stored under the same counter. Every change moves the counter and
    // stores the copy again. The hand-over is taken only when sessionStorage
    // can be read and neither record exists: a new window. Matching records
    // give the kept copy. Anything else (a copy that could not be stored, a
    // cleared name next to a kept copy, a counter with no copy or another
    // one's, sessionStorage unreadable) starts signed out and says why. It
    // never falls back to the launch-time values or to an older copy: either
    // may be someone who signed out since. The keychain still gets every
    // change (kc_set, kc_delete) for the next launch.
    var LIVE = "loca-desktop-credentials", MARK = "loca-desktop-credentials:";
    var generation = 0, lost = false;
    function keep() {
      generation += 1;
      try { window.name = MARK + generation; } catch (e) {}
      try {
        rawSet.call(window.sessionStorage, LIVE, JSON.stringify({ generation: generation, values: cache }));
      } catch (e) { /* the counters stay apart: the next load starts signed out */ }
    }
    var marked = typeof window.name === "string" && window.name.indexOf(MARK) === 0;
    var mark = marked ? Number(window.name.slice(MARK.length)) : NaN;
    var kept, readable = false;
    try { kept = rawGet.call(window.sessionStorage, LIVE); readable = true; } catch (e) {}
    if (readable && !marked && kept == null) {
      keep(); // a new window: the hand-over, recorded as taken
    } else {
      cache = {};
      lost = true;
      var live = null;
      try { live = JSON.parse(kept); } catch (e) {}
      if (mark > 0 && live && live.generation === mark && live.values && typeof live.values === "object") {
        KC_KEYS.forEach(function (k) { if (typeof live.values[k] === "string") cache[k] = live.values[k]; });
        generation = mark;
        lost = false;
      }
      if (lost) {
        // Start from nothing and record that, past both counters: if this
        // record cannot be stored either, the next load is lost again rather
        // than matching an older copy.
        generation = Math.max(mark > 0 ? mark : 0, live && live.generation > 0 ? live.generation : 0);
        keep();
      }
    }
    // Why this load starts signed out, kept for the sign-in view until the
    // person signs in again (a credential is stored, below): the web UI shows
    // it instead of its general door text (state.js lockRefusedSession).
    if (lost) {
      window.__LOCA_SIGNIN_NOTICE__ = "this window could not keep your sign-in across a reload, so it is signed out. Sign in again, or restart the app.";
      window.addEventListener("DOMContentLoaded", function () {
        try {
          var door = document.getElementById("doorline");
          if (door && window.__LOCA_SIGNIN_NOTICE__) door.textContent = window.__LOCA_SIGNIN_NOTICE__;
        } catch (e) {}
      });
    }

    proto.getItem = function (k) {
      if (isKc(this, k)) return Object.prototype.hasOwnProperty.call(cache, k) ? cache[k] : null;
      return rawGet.call(this, k);
    };
    proto.setItem = function (k, v) {
      if (isKc(this, k)) {
        cache[k] = String(v);
        keep();
        try { window.__LOCA_SIGNIN_NOTICE__ = null; } catch (e) {} // signed in again
        var fn = invoke();
        // If Tauri invoke isn't ready yet, keep it in the session cache only;
        // it is NEVER written to plain localStorage (no plaintext on disk).
        if (fn) { try { fn("kc_set", { key: k, value: String(v) }).catch(function () {}); } catch (e) {} }
        return;
      }
      return rawSet.call(this, k, v);
    };
    proto.removeItem = function (k) {
      if (isKc(this, k)) {
        delete cache[k];
        keep();
        var fn = invoke();
        if (fn) { try { fn("kc_delete", { key: k }).catch(function () {}); } catch (e) {} }
        return;
      }
      return rawRemove.call(this, k);
    };
  } catch (e) { /* fail open: never let the shim break the app */ }
})();
