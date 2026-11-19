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

LOCA_INFO_WIDE.addEventListener("change", placeLocaInfo);
new MutationObserver(placeLocaInfo).observe(document.documentElement, {
  attributes: true, attributeFilter: ["data-theme"],
});
