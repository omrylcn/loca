
(function () {
  try {
    window.__LOCA_NOTIFY__ = function (ev) {
      try {
        // Foreground dedup: a focused window already shows the event in-app, so
        // don't also raise an OS notification (no double-notify).
        if (document.hasFocus && document.hasFocus()) return;
        var t = window.__TAURI__;
        var invoke = (t && t.core && t.core.invoke) || (t && t.invoke);
        if (!invoke) return;
        invoke("notify", { payload: ev }).catch(function () {});
      } catch (e) {}
    };
  } catch (e) { /* never let the shim break the app */ }
})();
