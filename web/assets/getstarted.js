"use strict";
// First-open Getting Started guide + re-openable Help.
//
// Audience-aware: the guide AUTO-opens only in a Host context — the Desktop
// Host wrapper (window.__LOCA_HOST__) or a verified self-host Master
// (isAdmin()). An anonymous client/visitor is NEVER auto-shown it and is never
// told they are the Master; they can still open a general (client) view from
// the "?" Help button. The Desktop Host loads this SAME shared web UI, so Web
// self-host and Desktop Host behave identically — no desktop fork. Static
// content only: no session, no network, no secret.
(function () {
  function $(id) {
    return document.getElementById(id);
  }
  var SEEN_KEY = "loca-gs-seen";
  var PLAT_KEY = "loca-gs-plat";

  // Host context = the Desktop Host wrapper, or a verified Master session.
  // state/isAdmin are classic-script globals from state.js (bare identifiers,
  // not window properties), so reference them with typeof guards.
  function isHostContext() {
    try {
      if (window.__LOCA_HOST__ === true) return true;
      if (typeof isAdmin === "function" && isAdmin()) return true;
      if (typeof state === "object" && state && state.adminSession === true) return true;
    } catch (e) {}
    return false;
  }

  function open() {
    var o = $("gsOverlay");
    if (!o) return;
    // Show the Host view only in a Host context; everyone else gets the client
    // view, which carries no owner/Master framing.
    o.classList.toggle("as-host", isHostContext());
    o.classList.remove("hidden");
    var got = $("gsGot");
    if (got) {
      try {
        got.focus();
      } catch (e) {}
    }
  }

  function close(markSeen) {
    var o = $("gsOverlay");
    if (o) o.classList.add("hidden");
    if (markSeen) {
      try {
        localStorage.setItem(SEEN_KEY, "1");
      } catch (e) {}
    }
  }

  function seenBefore() {
    // If storage is unreadable, treat as NOT seen (a repeat beats a silent
    // miss); this only affects the Host contexts where auto-open is allowed.
    try {
      return localStorage.getItem(SEEN_KEY) === "1";
    } catch (e) {
      return false;
    }
  }

  function selectPlatform(plat) {
    var buttons = document.querySelectorAll(".gsplat button");
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].classList.toggle("on", buttons[i].getAttribute("data-plat") === plat);
    }
    var bodies = document.querySelectorAll("[data-plat-body]");
    for (var j = 0; j < bodies.length; j++) {
      bodies[j].classList.toggle("hidden", bodies[j].getAttribute("data-plat-body") !== plat);
    }
    try {
      localStorage.setItem(PLAT_KEY, plat);
    } catch (e) {}
  }

  window.addEventListener("load", function () {
    var help = $("helpBtn");
    if (help)
      help.addEventListener("click", function () {
        open();
      });

    var x = $("gsClose");
    if (x)
      x.addEventListener("click", function () {
        close(true);
      });

    var got = $("gsGot");
    if (got)
      got.addEventListener("click", function () {
        close(true);
      });

    var back = $("gsBackdrop");
    if (back)
      back.addEventListener("click", function () {
        close(true);
      });

    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape") {
        var o = $("gsOverlay");
        if (o && !o.classList.contains("hidden")) close(true);
      }
    });

    // Runtime toggle (Claude Code / Codex) inside the Host view.
    var platButtons = document.querySelectorAll(".gsplat button");
    for (var i = 0; i < platButtons.length; i++) {
      platButtons[i].addEventListener("click", function (e) {
        selectPlatform(e.currentTarget.getAttribute("data-plat"));
      });
    }
    var savedPlat = null;
    try {
      savedPlat = localStorage.getItem(PLAT_KEY);
    } catch (e) {}
    if (savedPlat) selectPlatform(savedPlat);

    // Auto-open is restricted to a Host context and shows once.
    if (seenBefore()) return;
    if (window.__LOCA_HOST__ === true) {
      open();
      return;
    }
    // A web self-host Master authenticates shortly after load — watch briefly
    // and auto-open once when a Host context is confirmed. A plain visitor
    // never trips this, so they are never auto-shown the guide.
    var tries = 0;
    var iv = setInterval(function () {
      if (seenBefore() || tries++ > 40) {
        clearInterval(iv);
        return;
      }
      if (isHostContext()) {
        clearInterval(iv);
        open();
      }
    }, 1500);
  });

  // Allow other code to reopen the guide programmatically.
  window.openGettingStarted = open;
})();
