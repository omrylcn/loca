"use strict";
// Building-wide search. The sidebar's field filters the list by name as one
// types (sidebar.js); Enter there, or the "Search all messages for “q”" line
// under it, asks the server for the words themselves: GET /search reads only
// the locas this person may enter (the same door as the list). The view lies
// over the loca's page and shows the hits grouped by loca, newest first; a hit
// opens its loca and, when the message is in the loaded tail, scrolls to it
// and marks it. Looks: search.css.

const SEARCH_LIMIT = 100;
let searchSeq = 0;      // the request whose answer may still be drawn
let searchJumpSeq = 0;  // the hit whose message is still being waited for
let searchHits = [];

function searchAllLabel(q) {
  return `Search all messages for “${q}”`;
}

// The line under the sidebar field: shown while it holds something.
function renderSideSearchAll() {
  const q = $("sideSearch").value.trim();
  const button = $("sideSearchAll");
  button.classList.toggle("hidden", !q);
  button.textContent = q ? searchAllLabel(q) : "";
}

function searchViewOpen() {
  return !$("searchView").classList.contains("hidden");
}

function openSearchView(q) {
  const query = String(q || "").trim();
  if (!query) return;
  $("searchView").classList.remove("hidden");
  $("searchQuery").value = query;
  setMobileSidebar(false);
  runBuildingSearch(query);
  $("searchQuery").focus();
}

function closeSearchView() {
  searchSeq++;
  $("searchView").classList.add("hidden");
}

// The text with every match of `q` marked, escaped. Folded the way the server
// folds (İ to i, then lower case), char by char so a match maps back.
function searchMarked(text, q) {
  const source = String(text || "");
  const fold = s => s.replace(/İ/g, "i").toLowerCase();
  const needle = fold(String(q || ""));
  if (!needle) return esc(source);
  const chars = Array.from(source);
  let folded = "";
  const origin = [];
  chars.forEach((ch, index) => {
    const f = fold(ch);
    for (let i = 0; i < f.length; i++) origin.push(index);
    folded += f;
  });
  let html = "";
  let from = 0;
  let at = folded.indexOf(needle);
  while (at !== -1) {
    const start = origin[at];
    const last = origin[at + needle.length - 1];
    if (start >= from) {
      html += esc(chars.slice(from, start).join("")) +
        `<mark>${esc(chars.slice(start, last + 1).join(""))}</mark>`;
      from = last + 1;
    }
    at = folded.indexOf(needle, at + needle.length);
  }
  return html + esc(chars.slice(from).join(""));
}

function searchWhen(ts) {
  const day = new Date(Number(ts)).toDateString();
  if (day === new Date().toDateString()) return `today ${fmtTime(ts)}`;
  return new Date(Number(ts)).toLocaleDateString([], { month: "short", day: "numeric" }) + ` ${fmtTime(ts)}`;
}

function searchGroupLabel(room) {
  if (isLobbyRoom(room)) return "Lobby";
  if (isDm(room)) return `@ ${roomLabel(room)}`;
  return `# ${roomLabel(room)}`;
}

function renderSearchResults(q, hits) {
  const count = hits.length >= SEARCH_LIMIT ? `${SEARCH_LIMIT}+` : String(hits.length);
  $("searchCount").textContent = `Messages ${count}`;
  const box = $("searchResults");
  box.innerHTML = "";
  if (!hits.length) {
    $("searchStatus").textContent = `Nothing said matches “${q}”.`;
    return;
  }
  $("searchStatus").textContent = "";
  // Grouped by loca, the loca with the newest hit first; newest first inside.
  const groups = new Map();
  for (const hit of hits) {
    if (!groups.has(hit.room)) groups.set(hit.room, []);
    groups.get(hit.room).push(hit);
  }
  for (const [room, list] of groups) {
    const section = document.createElement("section");
    section.className = "searchgroup";
    section.dataset.room = room;
    const items = list.map(hit => {
      const glyph = hit.sender_type === "agent" ? "*" : ".";
      const who = hit.sender_type === "agent" ? "searchagent" : "searchhuman";
      return `<li><button type="button" class="searchhit" data-search-room="${esc(hit.room)}" data-search-id="${Number(hit.id)}">` +
        `<span class="searchmeta"><span class="${who}">${glyph} ${esc(hit.sender)}</span> · ` +
        `<time datetime="${new Date(Number(hit.ts)).toISOString()}" title="${esc(fmtFull(hit.ts))}">${esc(searchWhen(hit.ts))}</time></span>` +
        `<span class="searchtext">${searchMarked(hit.text, q)}</span></button></li>`;
    }).join("");
    section.innerHTML = `<h3>${esc(searchGroupLabel(room))}</h3><ul class="searchhits">${items}</ul>`;
    box.appendChild(section);
  }
}

async function runBuildingSearch(q) {
  const seq = ++searchSeq;
  const gen = state.buildingGen;
  $("searchStatus").classList.remove("error");
  $("searchStatus").textContent = "Searching…";
  $("searchCount").textContent = "Messages";
  $("searchResults").innerHTML = "";
  try {
    const url = `${serverBase()}/search?q=${encodeURIComponent(q)}&limit=${SEARCH_LIMIT}`;
    const r = await fetch(url, { headers: adminHeaders({}) });
    if (seq !== searchSeq || buildingStale(gen)) return;
    if (!r.ok) {
      $("searchStatus").classList.add("error");
      $("searchStatus").textContent = r.status === 401
        ? "Your session ended; sign in again to search."
        : "Search did not answer; try again.";
      return;
    }
    const hits = await r.json();
    if (seq !== searchSeq || buildingStale(gen)) return;
    searchHits = Array.isArray(hits) ? hits : [];
    renderSearchResults(q, searchHits);
  } catch (error) {
    if (seq !== searchSeq || error?.name === "AbortError") return;
    $("searchStatus").classList.add("error");
    $("searchStatus").textContent = "Search did not answer; try again.";
  }
}

// Open the hit's loca and, once its history is drawn, go to the message.
// A message older than the loaded tail is not there: the loca simply opens.
async function openSearchHit(room, id) {
  closeSearchView();
  setMobileSidebar(false);
  const seq = ++searchJumpSeq;
  if (room !== state.room) joinRoom(room);
  if (state.tab !== "chat" && typeof switchTab === "function") switchTab("chat");
  const deadline = Date.now() + 8000;
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  while (Date.now() < deadline) {
    if (seq !== searchJumpSeq || state.room !== room) return;
    if (!joinedRoomNeedsBottom) {
      // The history is drawn. A message older than its tail is not in it:
      // the loca is open, and that is all.
      if (!document.querySelector(`#feed .row[data-id="${Number(id)}"]`)) return;
      // The history frame scrolls to the bottom again over a few frames
      // after it draws (chat.js scrollFeed); go to the message after those.
      await wait(300);
      if (seq !== searchJumpSeq || state.room !== room) return;
      const row = document.querySelector(`#feed .row[data-id="${Number(id)}"]`);
      if (!row) return;
      gotoMsg(Number(id));
      row.classList.add("searchfound");
      setTimeout(() => row.classList.remove("searchfound"), 4000);
      return;
    }
    await wait(120);
  }
}

$("sideSearch").addEventListener("input", renderSideSearchAll);
$("sideSearch").addEventListener("keydown", event => {
  if (event.key !== "Enter" || !event.target.value.trim()) return;
  event.preventDefault();
  openSearchView(event.target.value);
});
$("sideSearchAll").addEventListener("click", () => openSearchView($("sideSearch").value));
$("searchForm").addEventListener("submit", event => {
  event.preventDefault();
  const q = $("searchQuery").value.trim();
  if (q) runBuildingSearch(q);
});
$("searchClose").addEventListener("click", closeSearchView);
$("searchView").addEventListener("keydown", event => {
  if (event.key !== "Escape") return;
  event.stopPropagation();
  closeSearchView();
});
$("searchResults").addEventListener("click", event => {
  const hit = event.target.closest("[data-search-room]");
  if (hit) openSearchHit(hit.dataset.searchRoom, Number(hit.dataset.searchId));
});
// A sign-out: the view goes with it, so no one's hits outlast their page.
// (Opening a loca closes it too: socket.js joinRoom.)
new MutationObserver(() => {
  if (document.body.classList.contains("locked") && searchViewOpen()) {
    closeSearchView();
    $("searchResults").innerHTML = "";
    searchHits = [];
  }
}).observe(document.body, { attributes: true, attributeFilter: ["class"] });
