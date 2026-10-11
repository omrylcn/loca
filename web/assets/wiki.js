"use strict";
// Keep snapshots scoped to one room/context. Rendering uses textContent:
// wiki prose and source-linked quotations are data, never executable markup.
let wikiFetchSequence = 0;
let wikiSnapshot = null;
let wikiPageSlug = "overview";
let wikiEditDraft = null;

function wikiEditIdentity() {
  return { server: serverBase(), session: state.session, name: state.name, principal: state.principalId };
}

function wikiEditIdentityMatches(draft) {
  const current = wikiEditIdentity();
  return draft && Object.keys(current).every(key => current[key] === draft.identity[key]);
}

function stashWikiPageDraft() {
  if (wikiEditDraft && !$("wikiPageEdit").classList.contains("hidden")) {
    wikiEditDraft.title = $("wikiPageTitle").value;
    wikiEditDraft.body = $("wikiPageBody").value;
  }
}

function resetWikiForRoom() {
  stashWikiPageDraft();
  $("wikiPageEdit").classList.add("hidden");
  $("wikiPageTitle").value = ""; $("wikiPageBody").value = "";
  ++wikiFetchSequence; // Invalidate in-flight reads, including same-room reconnects.
  wikiSnapshot = null;
  wikiPageSlug = "overview";
  $("wikiPages").replaceChildren();
  $("wikiIndex").replaceChildren();
  $("wikiStatus").textContent = state.room ? "Loading wiki…" : "Select a loca.";
  $("wikiEditor").disabled = false;
  toggleWikiSettings(false);
  renderWikiEditor();
}

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
  $("wikiEditPage").classList.toggle("hidden", !allowed || !wikiSnapshot);
  if (!wikiEditIdentityMatches(wikiEditDraft)) wikiEditDraft = null;
  if (!allowed || !wikiEditDraft || wikiEditDraft.room !== state.room) {
    $("wikiPageEdit").classList.add("hidden");
    $("wikiPageTitle").value = ""; $("wikiPageBody").value = "";
  }
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
  if (wikiSnapshot && wikiSnapshot.room !== state.room) return;
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
    link.onclick = () => { stashWikiPageDraft(); $("wikiPageEdit").classList.add("hidden"); wikiPageSlug = page.slug; renderWikiPages(); }; $("wikiIndex").append(link);
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

function beginWikiPageEdit() {
  if (!isLocaOperator() || wikiSnapshot?.room !== state.room) return;
  stashWikiPageDraft();
  if (wikiEditDraft && wikiEditIdentityMatches(wikiEditDraft) &&
      (wikiEditDraft.room !== state.room || wikiEditDraft.slug !== wikiPageSlug)) {
    $("wikiStatus").textContent = "A draft is still open. Return to its page and save or discard it first.";
    return;
  }
  const page = wikiSnapshot.pages.find(value => value.slug === wikiPageSlug);
  if (!page) return;
  if (!wikiEditDraft) wikiEditDraft = { identity: wikiEditIdentity(), room: state.room,
    slug: page.slug, revision: wikiSnapshot.revision, title: page.title, body: page.body,
    original: { title: page.title, body: page.body, revision: page.revision, sources: [...page.sources] } };
  wikiEditDraft.context = state.locaContext;
  $("wikiPageTitle").value = wikiEditDraft.title;
  $("wikiPageBody").value = wikiEditDraft.body;
  $("wikiEditStatus").textContent = `Draft based on revision ${wikiEditDraft.revision}.`;
  $("wikiPageEdit").classList.remove("hidden");
  $("wikiPageBody").focus();
}

function cancelWikiPageEdit() {
  wikiEditDraft = null;
  $("wikiPageEdit").classList.add("hidden");
  $("wikiPageTitle").value = ""; $("wikiPageBody").value = "";
}

async function saveWikiPageEdit() {
  stashWikiPageDraft();
  const draft = wikiEditDraft;
  const current = () => wikiEditDraft === draft && wikiEditIdentityMatches(draft) &&
    draft.room === state.room && draft.context === state.locaContext && isLocaOperator();
  if (!current() || $("wikiPageSave").disabled) return;
  const title = draft.title, body = draft.body;
  const changesContent = title !== draft.original.title || body !== draft.original.body || draft.original.sources.length !== 0;
  if (!title.trim() || new TextEncoder().encode(title).length > 256 || new TextEncoder().encode(body).length > 32768) {
    $("wikiEditStatus").textContent = "Use a title up to 256 bytes and a page up to 32 KiB. Your draft is kept."; return;
  }
  $("wikiPageSave").disabled = true;
  $("wikiPageCancel").disabled = true;
  // Freeze the submitted values until verification; later typing must not be
  // mistaken for what was accepted by the server.
  $("wikiPageTitle").readOnly = true; $("wikiPageBody").readOnly = true;
  $("wikiEditStatus").textContent = "Saving and checking this page…";
  const url = `${draft.identity.server}/rooms/${encodeURIComponent(draft.room)}/wiki/pages/${encodeURIComponent(draft.slug)}`;
  try {
    const response = await fetch(url, { method: "PUT", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ expected_revision: draft.revision, title, body }) });
    if (!current()) return;
    if (response.status === 409) throw new Error("Wiki changed while you were editing. Your draft is kept; refresh and compare before saving again.");
    if (!response.ok) throw new Error(`Could not save (HTTP ${response.status}). Your draft is kept.`);
    const receipt = await response.json();
    if (!current()) return;
    if (receipt.room !== draft.room || receipt.slug !== draft.slug || !Number.isSafeInteger(receipt.revision) ||
        receipt.revision !== draft.revision + Number(changesContent)) throw new Error("Save receipt could not be verified. Your draft is kept.");
    const read = await fetch(url, { headers: adminHeaders() });
    if (!current()) return;
    if (!read.ok) throw new Error("Save was accepted but readback failed. Your draft is kept; do not blindly resend.");
    const saved = await read.json();
    if (!current()) return;
    const expectedPageRevision = changesContent ? receipt.revision : draft.original.revision;
    if (saved.room !== draft.room || saved.page?.slug !== draft.slug || saved.page.revision !== expectedPageRevision ||
        saved.page.title !== title || saved.page.body !== body || saved.page.sources?.length !== 0) {
      throw new Error("Save was accepted but the current page differs. Your draft is kept; refresh and compare.");
    }
    cancelWikiPageEdit();
    await fetchWiki();
    if (state.room === draft.room && state.locaContext === draft.context && wikiEditIdentityMatches(draft))
      $("wikiStatus").textContent = `Page saved and verified · revision ${receipt.revision}. Review progress was not changed.`;
  } catch (error) {
    if (current()) $("wikiEditStatus").textContent = String(error.message || error);
  } finally {
    $("wikiPageSave").disabled = false; $("wikiPageCancel").disabled = false;
    $("wikiPageTitle").readOnly = false; $("wikiPageBody").readOnly = false;
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
    $("wikiAutoEnabled").checked = !!wiki.enabled;
    $("wikiInterval").value = wiki.interval_messages;
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

$("wikiEditPage").addEventListener("click", beginWikiPageEdit);
$("wikiPolicy").addEventListener("submit", async event => {
  event.preventDefault();
  if (!isLocaOperator() || wikiSnapshot?.room !== state.room) return;
  const room = state.room, context = state.locaContext, identity = wikiEditIdentity();
  const current = () => room === state.room && context === state.locaContext &&
    JSON.stringify(identity) === JSON.stringify(wikiEditIdentity()) && isLocaOperator();
  const interval = Number($("wikiInterval").value), enabled = $("wikiAutoEnabled").checked;
  if (!Number.isInteger(interval) || interval < 1 || interval > 1000) {
    $("wikiPolicyStatus").textContent = "Choose 1–1000 messages."; return;
  }
  $("wikiPolicySave").disabled = true;
  try {
    const response = await fetch(`${identity.server}/rooms/${encodeURIComponent(room)}/wiki/config`, {
      method: "PUT", headers: { ...adminHeaders(), "content-type": "application/json" },
      body: JSON.stringify({ editor: wikiSnapshot.editor_name, enabled, interval_messages: interval })
    });
    if (!current()) return;
    if (response.status === 409) throw new Error("Editor worker is not ready. Reconnect its native runtime, then save again.");
    if (!response.ok) throw new Error(`Could not save review settings (${response.status}).`);
    await fetchWiki();
    if (!current()) return;
    if (wikiSnapshot?.enabled !== enabled || wikiSnapshot?.interval_messages !== interval)
      throw new Error("Settings acceptance could not be verified. Refresh before retrying.");
    $("wikiPolicyStatus").textContent = enabled ? "Automatic review enabled and verified." : "Automatic review paused and verified.";
  } catch (error) { if (current()) $("wikiPolicyStatus").textContent = error.message; }
  finally { $("wikiPolicySave").disabled = false; }
});

$("wikiShare").addEventListener("submit", async event => {
  event.preventDefault();
  if (!isLocaOperator() || wikiSnapshot?.room !== state.room) return;
  const room = state.room, context = state.locaContext, identity = wikiEditIdentity(), slug = wikiPageSlug;
  const target = $("wikiShareTarget").value.trim();
  const current = () => room === state.room && context === state.locaContext &&
    JSON.stringify(identity) === JSON.stringify(wikiEditIdentity());
  $("wikiShareSend").disabled = true;
  try {
    const response = await fetch(`${identity.server}/rooms/${encodeURIComponent(room)}/wiki/context/offer`, {
      method:"POST", headers:{...adminHeaders(),"content-type":"application/json"},
      body:JSON.stringify({target,slugs:[slug],reason:"operator selected this page"})
    });
    if (!current()) return;
    if (!response.ok) throw new Error(`Page selection failed (${response.status}).`);
    const receipt = await response.json();
    if (!current()) return;
    if (receipt.offered !== true) throw new Error("Selection receipt unavailable.");
    $("wikiShareStatus").textContent = `Selected for @${target}. Model-context acceptance is not yet confirmed.`;
  } catch(error) { if(current()) $("wikiShareStatus").textContent=error.message; }
  finally { $("wikiShareSend").disabled=false; }
});
$("wikiPageCancel").addEventListener("click", cancelWikiPageEdit);
$("wikiPageEdit").addEventListener("submit", event => { event.preventDefault(); saveWikiPageEdit(); });

$("wikiPanel").addEventListener("keydown", event => {
  if (event.key === "Escape" && !$("wikiConfig").classList.contains("hidden")) {
    toggleWikiSettings(false); $("wikiSettingsToggle").focus();
  }
});
