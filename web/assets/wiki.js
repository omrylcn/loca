"use strict";
// Keep snapshots scoped to one room/context. Rendering uses textContent:
// wiki prose and source-linked quotations are data, never executable markup.
let wikiFetchSequence = 0;
let wikiSnapshot = null;
let wikiPageSlug = "overview";

function toggleWikiSettings(open = $("wikiConfig").classList.contains("hidden")) {
  open = open && isLocaOperator();
  $("wikiConfig").classList.toggle("hidden", !open);
  $("wikiSettingsToggle").setAttribute("aria-expanded", String(open));
  if (open) $("wikiSettingsClose").focus();
}

function renderWikiEditor() {
  const allowed = isLocaOperator() && !!state.room;
  $("wikiEditorControl").classList.toggle("hidden", !allowed);
  $("wikiSettingsToggle").classList.toggle("hidden", !allowed);
  if (!allowed) toggleWikiSettings(false);
  const current = wikiSnapshot?.room === state.room ? wikiSnapshot.editor_name || "" : "";
  const members = [...(state.members || []), ...(state.seatedAway || [])];
  const names = [...new Set(members.filter(m => (m.type || m.kind) === "agent").map(m => m.name))].sort((a, b) => a.localeCompare(b));
  if (current && !names.includes(current)) names.push(current);
  const select = $("wikiEditor");
  select.replaceChildren(new Option(current ? "Choose another editor" : "Not assigned", ""));
  if (current) select.options[0].disabled = true;
  for (const name of names) select.add(new Option(name, name));
  select.value = current;
  $("wikiEditorName").textContent = allowed ? "" : current ? `Editor · ${current}` : "";
}

function renderWikiPages() {
  $("wikiPages").replaceChildren(); $("wikiIndex").replaceChildren();
  const pages = [...(wikiSnapshot?.pages || [])].sort((a, b) => {
    const order = { overview: 0, working: 1 };
    return (order[a.slug] ?? 2) - (order[b.slug] ?? 2) || a.title.localeCompare(b.title);
  });
  if (!pages.some(p => p.slug === wikiPageSlug)) wikiPageSlug = pages[0]?.slug || "overview";
  const empty = !pages.some(p => p.body.trim());
  if (empty) {
    const box = document.createElement("div"); box.className = "wikiempty";
    const glyph = document.createElement("span"); glyph.className = "wikiemptyglyph"; glyph.textContent = "◇";
    const title = document.createElement("h3"); title.textContent = "A shared place for what matters.";
    const description = document.createElement("p");
    description.textContent = wikiSnapshot ? "Your editor is assigned. No published content yet." : "This loca's wiki has not been prepared yet.";
    box.append(glyph, title, description);
    if (!wikiSnapshot && isLocaOperator()) {
      const button = document.createElement("button"); button.className = "wikiprimary"; button.textContent = "Choose an editor";
      button.onclick = () => $("wikiEditor").focus(); box.append(button);
    }
    $("wikiPages").append(box);
  }
  for (const page of pages) {
    const link = document.createElement("button"); link.type = "button"; link.textContent = page.title;
    link.className = page.slug === wikiPageSlug ? "active" : "";
    link.setAttribute("aria-current", page.slug === wikiPageSlug ? "page" : "false");
    link.onclick = () => { wikiPageSlug = page.slug; renderWikiPages(); }; $("wikiIndex").append(link);
    if (page.slug !== wikiPageSlug || empty) continue;
    const section = document.createElement("article"); section.className = "wikidocument";
    const heading = document.createElement("h1"); heading.textContent = page.title;
    const body = document.createElement("div"); body.className = "markdown wikibody";
    body.innerHTML = renderMarkdown(page.body || "No content yet."); section.append(heading, body);
    if (page.sources.length) {
      const sources = document.createElement("footer"); sources.className = "wikisources"; sources.append("Source messages ");
      for (const id of page.sources) {
        const button = document.createElement("button"); button.textContent = `#${id}`;
        button.onclick = () => { switchTab("chat"); gotoMsg(id); }; sources.append(button);
      }
      section.append(sources);
    }
    $("wikiPages").append(section);
  }
}

async function fetchWiki() {
  const room = state.room, context = state.locaContext;
  const sequence = ++wikiFetchSequence;
  const current = () => room === state.room && context === state.locaContext && sequence === wikiFetchSequence;
  $("wikiPages").replaceChildren();
  $("wikiIndex").replaceChildren();
  if (wikiSnapshot?.room !== room) { wikiSnapshot = null; wikiPageSlug = "overview"; toggleWikiSettings(false); }
  renderWikiEditor();
  $("wikiStatus").textContent = "Loading wiki…";
  if (!room) { $("wikiStatus").textContent = "Select a loca."; return; }
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/wiki`, { headers: adminHeaders() });
    if (!current()) return;
    if (response.status === 404) {
      wikiSnapshot = null; $("wikiStatus").textContent = "Shared context · No published pages";
      renderWikiEditor(); renderWikiPages(); return;
    }
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    const wiki = await response.json();
    if (!current()) return;
    if (wiki.room !== room) throw new Error("Wiki response belongs to another loca.");
    $("wikiStatus").textContent = `Revision ${wiki.revision} · last review: ${wiki.reviewed_at ? fmtFull(wiki.reviewed_at) : "never"} · last edit: ${wiki.edited_at ? fmtFull(wiki.edited_at) : "never"} · reviewed through message ${wiki.reviewed_through}`;
    wikiSnapshot = wiki;
    renderWikiEditor(); renderWikiPages();
  } catch (error) {
    if (current()) $("wikiStatus").textContent = String(error.message || error);
  }
}

async function configureWiki() {
  if (!isLocaOperator()) return;
  const room = state.room, context = state.locaContext;
  const editor = $("wikiEditor").value, interval = wikiSnapshot?.interval_messages || 30;
  if (!editor) { renderWikiEditor(); return; }
  $("wikiEditor").disabled = true;
  try {
    const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/wiki/config`, {
      method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ editor, enabled: false, interval_messages: interval }),
    });
    if (room !== state.room || context !== state.locaContext) return;
    if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
    await fetchWiki();
  } catch (error) {
    if (room === state.room && context === state.locaContext) { renderWikiEditor(); $("wikiStatus").textContent = String(error.message || error); }
  } finally { $("wikiEditor").disabled = false; }
}

$("wikiPanel").addEventListener("keydown", event => {
  if (event.key === "Escape" && !$("wikiConfig").classList.contains("hidden")) {
    toggleWikiSettings(false); $("wikiSettingsToggle").focus();
  }
});
