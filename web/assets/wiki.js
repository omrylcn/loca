"use strict";
// Keep snapshots scoped to one room/context. Rendering uses textContent:
// wiki prose and source-linked quotations are data, never executable markup.
let wikiFetchSequence = 0;

async function fetchWiki() {
  const room = state.room, context = state.locaContext;
  const sequence = ++wikiFetchSequence;
  const current = () => room === state.room && context === state.locaContext && sequence === wikiFetchSequence;
  $("wikiPages").replaceChildren();
  $("wikiIndex").replaceChildren();
  $("wikiConfig").classList.toggle("hidden", !isLocaOperator());
  $("wikiStatus").textContent = "Loading wiki…";
  if (!room) { $("wikiStatus").textContent = "Select a loca."; return; }
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/wiki`, { headers: adminHeaders() });
    if (!current()) return;
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    const wiki = await response.json();
    if (!current()) return;
    if (wiki.room !== room) throw new Error("Wiki response belongs to another loca.");
    $("wikiStatus").textContent = `Revision ${wiki.revision} · last review: ${wiki.reviewed_at ? fmtFull(wiki.reviewed_at) : "never"} · last edit: ${wiki.edited_at ? fmtFull(wiki.edited_at) : "never"} · reviewed through message ${wiki.reviewed_through}`;
    $("wikiInterval").value = wiki.interval_messages;
    for (const page of wiki.pages) {
      const section = document.createElement("section");
      section.className = "memoryeditor";
      section.id = `wiki-page-${page.slug}`;
      const heading = document.createElement("h3"); heading.textContent = page.title;
      const body = document.createElement("div"); body.className = "memorybody";
      body.style.whiteSpace = "pre-wrap"; body.textContent = page.body || "No content yet.";
      const sources = document.createElement("div"); sources.className = "sysline";
      sources.append("Sources: ");
      for (const id of page.sources) {
        const button = document.createElement("button"); button.textContent = `#${id}`;
        button.onclick = () => { switchTab("chat"); gotoMsg(id); };
        sources.append(button);
      }
      if (!page.sources.length) sources.append("none supplied");
      section.append(heading, body, sources); $("wikiPages").append(section);
      const link = document.createElement("a"); link.href = `#${section.id}`;
      link.textContent = `${page.title} `; $("wikiIndex").append(link);
    }
  } catch (error) {
    if (current()) $("wikiStatus").textContent = String(error.message || error);
  }
}

async function configureWiki() {
  if (!isLocaOperator()) return;
  const room = state.room, context = state.locaContext;
  const editor = $("wikiEditor").value.trim(), interval = Number($("wikiInterval").value);
  if (!editor || !Number.isInteger(interval) || interval < 1 || interval > 1000) {
    $("wikiStatus").textContent = "Enter an exact agent name and an interval from 1 to 1000."; return;
  }
  $("wikiConfigure").disabled = true;
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/wiki/config`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ editor, enabled: false, interval_messages: interval }),
    });
    if (room !== state.room || context !== state.locaContext) return;
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    await fetchWiki();
  } catch (error) {
    if (room === state.room && context === state.locaContext) $("wikiStatus").textContent = String(error.message || error);
  } finally { $("wikiConfigure").disabled = false; }
}
