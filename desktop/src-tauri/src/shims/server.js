
(function () {
  try {
    // Host mode (standalone flavor) is LOCKED to its bundled local server: a
    // saved remote override must never apply and manual changes are not
    // persisted, so the app can't be pointed away from its own sidecar.
    var locked = !!window.__LOCA_LOCK_SERVER__;
    // In the client flavor, a server the user saved before wins over the baked
    // default.
    var saved = null;
    try { saved = localStorage.getItem("loca-desktop-server"); } catch (e) {}
    if (!locked && saved && saved.trim()) { window.__LOCA_DEFAULT_SERVER__ = saved.trim(); }
    if (!locked) {
      // Persist future manual edits to the #server field once it exists.
      window.addEventListener("load", function () {
        try {
          var input = document.getElementById("server");
          if (!input) return;
          input.addEventListener("change", function () {
            try { localStorage.setItem("loca-desktop-server", (input.value || "").trim()); } catch (e) {}
          });
        } catch (e) {}
      });
    }
  } catch (e) { /* never let the shim break the app */ }
})();
