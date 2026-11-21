"use strict";
// Loca info beside the chat. On a screen wide enough for three columns, This Loca — who answers for the
// loca, who is at the table, calling someone in — stands on the right of the
// chat instead of behind the sidebar's second tab, and the sidebar keeps
// Your Locas. It is the same panel (the same elements, moved), so everything
// that draws or reads it keeps working; narrower screens keep the two
// sidebar tabs.

const LOCA_INFO_WIDE = window.matchMedia("(min-width: 1400px)");

function locaInfoWanted() {
  return LOCA_INFO_WIDE.matches && !!state.room && !isDm(state.room) && !isBuildingView();
}

function placeLocaInfo() {
  const view = $("sideLocaView");
  const host = $("locaInfo");
  if (!view || !host) return;
  const on = locaInfoWanted();
  const was = document.body.classList.contains("locainfo-on");
  document.body.classList.toggle("locainfo-on", on);
  host.classList.toggle("hidden", !on);
  if (on && view.parentElement !== host) host.appendChild(view);
  if (!on && view.parentElement === host) $("sidebar").appendChild(view);
  // The sidebar shows Your Locas while the panel is on the right; when it
  // goes back, the sidebar tab that is selected decides again.
  if (on !== was) setSidebarView(on ? "building" : state.sidebarView || "building");
  // An open thread stands in This Loca's place, or over the chat (threads.js).
  if (typeof placeThreadPanel === "function") placeThreadPanel();
}

// The panel's width: its left edge is a grip (pointer or arrow keys), kept
// within 320px and 560px (and leaving the chat room), remembered here.
const LOCA_INFO_WIDTH_KEY = "loca-locainfo-width";
const LOCA_INFO_MIN = 320;
function locaInfoMax() { return Math.max(LOCA_INFO_MIN, Math.min(560, innerWidth - 280 - 520)); }
function setLocaInfoWidth(width, persist = false) {
  const bounded = Math.round(Math.max(LOCA_INFO_MIN, Math.min(locaInfoMax(), width)));
  document.documentElement.style.setProperty("--locainfo-w", `${bounded}px`);
  const grip = $("locaInfoResize");
  if (grip) {
    grip.setAttribute("aria-valuenow", String(bounded));
    grip.setAttribute("aria-valuemax", String(Math.round(locaInfoMax())));
  }
  if (persist) { try { localStorage.setItem(LOCA_INFO_WIDTH_KEY, String(bounded)); } catch (e) {} }
}
(() => {
  const host = $("locaInfo");
  if (!host) return;
  const grip = document.createElement("div");
  grip.className = "locainforesize";
  grip.id = "locaInfoResize";
  grip.tabIndex = 0;
  grip.setAttribute("role", "separator");
  grip.setAttribute("aria-orientation", "vertical");
  grip.setAttribute("aria-label", "Resize Loca info");
  grip.setAttribute("aria-valuemin", String(LOCA_INFO_MIN));
  host.prepend(grip);
  let saved = 0;
  try { saved = Number(localStorage.getItem(LOCA_INFO_WIDTH_KEY) || 0); } catch (e) {}
  if (saved) setLocaInfoWidth(saved);
  let startX = 0, startWidth = 0;
  grip.onpointerdown = (e) => {
    startX = e.clientX;
    startWidth = host.getBoundingClientRect().width;
    grip.setPointerCapture(e.pointerId);
    grip.classList.add("dragging");
  };
  grip.onpointermove = (e) => {
    if (!grip.hasPointerCapture(e.pointerId)) return;
    setLocaInfoWidth(startWidth + startX - e.clientX);
  };
  grip.onpointerup = (e) => {
    if (grip.hasPointerCapture(e.pointerId)) grip.releasePointerCapture(e.pointerId);
    grip.classList.remove("dragging");
    setLocaInfoWidth(host.getBoundingClientRect().width, true);
  };
  grip.onkeydown = (event) => {
    // The grip is on the panel's left edge: left widens, right narrows.
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const current = host.getBoundingClientRect().width;
    if (event.key === "ArrowLeft") setLocaInfoWidth(current + 24, true);
    else if (event.key === "ArrowRight") setLocaInfoWidth(current - 24, true);
    else if (event.key === "Home") setLocaInfoWidth(LOCA_INFO_MIN, true);
    else setLocaInfoWidth(locaInfoMax(), true);
  };
})();

LOCA_INFO_WIDE.addEventListener("change", placeLocaInfo);
new MutationObserver(placeLocaInfo).observe(document.documentElement, {
  attributes: true, attributeFilter: ["data-theme"],
});
