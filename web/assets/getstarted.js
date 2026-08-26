"use strict";
// First-open Getting Started guide + re-openable Help.
//
// Shows once automatically on a browser's first visit (localStorage-gated) and
// can be reopened anytime from the "?" Help button. The Desktop Host loads this
// SAME shared web UI, so there is no desktop fork — Web self-host Master and
// Desktop Host behave identically. Static content only: no secrets, no network.
(function () {
  function $(id) {
    return document.getElementById(id);
  }
  var SEEN_KEY = "loca-gs-seen";

  function open() {
    var o = $("gsOverlay");
    if (!o) return;
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
    // If storage is unreadable, treat as NOT seen: a repeat guide beats a
    // silent miss on someone's genuine first open.
    try {
      return localStorage.getItem(SEEN_KEY) === "1";
    } catch (e) {
      return false;
    }
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

    // First visit on this browser: show the guide once.
    if (!seenBefore()) open();
  });

  // Allow other code to reopen the guide programmatically.
  window.openGettingStarted = open;
})();
