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

  // The Desktop Host injects the real local Skill Library path (and any prep
  // error) at boot.
  function skillLibraryPath() {
    try {
      return window.__LOCA_SKILL_LIBRARY__ || null;
    } catch (e) {
      return null;
    }
  }
  function skillLibraryError() {
    try {
      return window.__LOCA_SKILL_LIBRARY_ERROR__ || null;
    } catch (e) {
      return null;
    }
  }
  // An IDEMPOTENT install command for one or more skills into `dir` — safe to
  // re-run for a fresh install OR an update, and it never nests `loca/loca`.
  // PRIMARY: copy from the local read-only library (offline). FALLBACK (web, or
  // no local path): download from this Host, the same bytes /downloads/skills
  // serves. No credential, no repo, no git.
  function installCmd(dir, skills) {
    var lib = skillLibraryPath();
    var parts = skills.map(function (s) {
      if (lib) {
        // Quote the library path — App Data can contain spaces (e.g. macOS).
        return "rm -rf " + dir + "/" + s + ' && cp -R "' + lib + "/" + s + '" ' + dir + "/" + s;
      }
      var origin = (window.location && window.location.origin) || "";
      return (
        "curl -fsSL " + origin + "/downloads/skills/" + s + " -o /tmp/" + s + ".zip && " +
        "rm -rf " + dir + "/" + s + " && unzip -oq /tmp/" + s + ".zip -d " + dir
      );
    });
    return "mkdir -p " + dir + " && " + parts.join(" && ");
  }
  function fillInstallCommands() {
    var set = function (id, dir, skills) {
      var el = $(id);
      if (el) el.textContent = installCmd(dir, skills);
    };
    // Agent: loca only. Caretaker: loca + loca-care.
    set("gsInstallClaude", "~/.claude/skills", ["loca"]);
    set("gsInstallCodex", "~/.codex/skills", ["loca"]);
    set("gsInstallClaudeCare", "~/.claude/skills", ["loca", "loca-care"]);
    set("gsInstallCodexCare", "~/.codex/skills", ["loca", "loca-care"]);

    var lib = skillLibraryPath();
    var pathEl = $("gsLibPath");
    if (pathEl) {
      if (lib) {
        pathEl.textContent = "Your Skill Library: " + lib;
        pathEl.classList.remove("hidden");
      } else {
        pathEl.classList.add("hidden");
      }
    }
    // Visible health status when the local library could not be prepared.
    var errEl = $("gsLibError");
    if (errEl) {
      var err = skillLibraryError();
      if (err && !lib) {
        errEl.textContent =
          "Skill Library unavailable: " + err + " — using the download command below.";
        errEl.classList.remove("hidden");
      } else {
        errEl.classList.add("hidden");
      }
    }
  }

  function selectTab(name) {
    var tabs = document.querySelectorAll(".gstab");
    for (var i = 0; i < tabs.length; i++) {
      tabs[i].classList.toggle("on", tabs[i].getAttribute("data-gstab") === name);
    }
    var panels = document.querySelectorAll("[data-gspanel]");
    for (var j = 0; j < panels.length; j++) {
      panels[j].classList.toggle("hidden", panels[j].getAttribute("data-gspanel") !== name);
    }
  }

  function copyFrom(id, button) {
    var el = $(id);
    if (!el) return;
    var text = el.textContent || "";
    var done = function () {
      if (!button) return;
      var was = button.textContent;
      button.textContent = "Copied";
      setTimeout(function () {
        button.textContent = was || "Copy";
      }, 1200);
    };
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, function () {});
        return;
      }
    } catch (e) {}
    // Fallback for insecure/older contexts: select the text and execCommand.
    try {
      var range = document.createRange();
      range.selectNodeContents(el);
      var sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      document.execCommand("copy");
      sel.removeAllRanges();
      done();
    } catch (e2) {}
  }

  function open() {
    var o = $("gsOverlay");
    if (!o) return;
    // Show the Host view only in a Host context; everyone else gets the client
    // view, which carries no owner/Master framing.
    o.classList.toggle("as-host", isHostContext());
    o.classList.remove("hidden");
    // Point the install commands at the current Host origin every time it opens.
    fillInstallCommands();
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

    // Tabbed guide: switch panels on tab click.
    var tabButtons = document.querySelectorAll(".gstab");
    for (var t = 0; t < tabButtons.length; t++) {
      tabButtons[t].addEventListener("click", function (e) {
        selectTab(e.currentTarget.getAttribute("data-gstab"));
      });
    }
    // Copy the install command to the clipboard (a copy button, never an
    // installer — the agent/user runs the command).
    var copyButtons = document.querySelectorAll(".gscopy");
    for (var c = 0; c < copyButtons.length; c++) {
      copyButtons[c].addEventListener("click", function (e) {
        copyFrom(e.currentTarget.getAttribute("data-copy"), e.currentTarget);
      });
    }
    fillInstallCommands();

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
