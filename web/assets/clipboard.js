"use strict";
// Copying a secret or an address to the clipboard, on any Building address.

/// Copy text to the clipboard. The Clipboard API exists only in a secure
/// context (HTTPS or localhost); a Building reached over plain http on a VPN
/// address has none, so fall back to a hidden textarea and execCommand. If
/// even that fails, select `el`'s text so Ctrl+C copies it. True when copied.
async function copyText(text, el) {
  if (window.isSecureContext && navigator.clipboard?.writeText) {
    try { await navigator.clipboard.writeText(text); return true; } catch (e) { /* fall through */ }
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;";
  document.body.appendChild(area);
  area.select();
  let ok = false;
  try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
  area.remove();
  if (!ok && el) {
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  }
  return ok;
}
