"use strict";
const reactionByMessage = new Map();
const locaReactionSet = ["✓", "✦", "!", "♥"];

function reactionKey(messageId, emoji) { return `${messageId}\u0000${emoji}`; }
function reactionHtml(messageId) {
  const marks = locaReactionSet.map(emoji => {
    const actors = reactionByMessage.get(reactionKey(messageId, emoji)) || [];
    if (!actors.length) return "";
    const mine = actors.includes(state.name);
    return `<button type="button" class="reactionchip${mine ? " mine" : ""}" data-react="${messageId}" data-emoji="${emoji}" title="${esc(actors.join(", "))}">${emoji}<span>${actors.length}</span></button>`;
  }).join("");
  return `<div class="reactions" data-reactions="${messageId}">${marks}</div>`;
}
function renderMessageReactions(messageId) {
  const host = document.querySelector(`[data-reactions="${messageId}"]`);
  if (host) host.outerHTML = reactionHtml(messageId);
}
function applyReactionSummary(reaction) {
  reactionByMessage.set(reactionKey(reaction.message_id, reaction.emoji), reaction.actors || []);
  renderMessageReactions(reaction.message_id);
}
async function fetchReactions() {
  if (!state.room) return;
  const room = state.room;
  const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(room)}/reactions`, { headers: adminHeaders({}) });
  if (!response.ok || room !== state.room) return;
  reactionByMessage.clear();
  for (const reaction of await response.json()) applyReactionSummary(reaction);
}
async function setReaction(messageId, emoji) {
  const message = msgById.get(Number(messageId));
  if (!message || message.sender === state.name) return;
  const actors = reactionByMessage.get(reactionKey(messageId, emoji)) || [];
  const active = !actors.includes(state.name);
  const response = await fetch(`${serverBase()}/rooms/${encodeURIComponent(state.room)}/messages/${messageId}/reactions`, {
    method: "POST",
    headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ emoji, active, reactor: state.name, reactor_type: "user" }),
  });
  if (!response.ok) {
    addSys(`reaction failed: ${await response.text()}`);
    return;
  }
  // Render the server-confirmed summary immediately. Previously the UI waited
  // only for the WebSocket echo, so a delayed/missed echo made a successful
  // reaction look as if nothing happened. A later WS frame is idempotent.
  applyReactionSummary(await response.json());
}
// Safe Markdown rendering and the live chat feed.
// Notes are durable shared memory, so Markdown is rendered without trusting
// author-supplied HTML. Only a small, readable subset is supported; every
// other character is escaped before it reaches innerHTML.
function markdownHref(href) {
  return /^(https?:\/\/|mailto:|\/(?!\/)|#)/i.test(String(href));
}

function renderMarkdownInline(text) {
  const source = String(text);
  const pattern = /(`[^`\n]+`|\[[^\]\n]+\]\([^)\s]+\))/g;
  let html = "";
  let cursor = 0;

  const renderPlain = (plain) => esc(plain)
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_\n]+)__/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])\*([^*\n]+)\*/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_([^_\n]+)_/g, "$1<em>$2</em>");

  for (const match of source.matchAll(pattern)) {
    html += renderPlain(source.slice(cursor, match.index));
    const token = match[0];
    if (token.startsWith("`")) {
      html += `<code class="inline">${esc(token.slice(1, -1))}</code>`;
    } else {
      const link = token.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (link && markdownHref(link[2])) {
        html += `<a href="${esc(link[2])}" target="_blank" rel="noopener noreferrer">${esc(link[1])}</a>`;
      } else {
        html += renderPlain(token);
      }
    }
    cursor = match.index + token.length;
  }
  return html + renderPlain(source.slice(cursor));
}

function markdownTableCells(line) {
  const trimmed = String(line).trim().replace(/^\|/, "").replace(/\|$/, "");
  return trimmed.includes("|") ? trimmed.split("|").map(cell => cell.trim()) : [];
}

function renderMarkdown(text) {
  const lines = String(text || "").replace(/\r\n?/g, "\n").split("\n");
  const html = [];
  let paragraph = [];
  let listType = null;
  let listItems = [];
  let quote = [];
  let code = null;

  const flushParagraph = () => {
    if (paragraph.length) html.push(`<p>${renderMarkdownInline(paragraph.join(" "))}</p>`);
    paragraph = [];
  };
  const flushList = () => {
    if (listType) html.push(`<${listType}>${listItems.map(item => `<li>${renderMarkdownInline(item)}</li>`).join("")}</${listType}>`);
    listType = null;
    listItems = [];
  };
  const flushQuote = () => {
    if (quote.length) html.push(`<blockquote>${quote.map(renderMarkdownInline).join("<br>")}</blockquote>`);
    quote = [];
  };
  const flushText = () => { flushParagraph(); flushList(); flushQuote(); };

  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const line = lines[lineIndex];
    if (code !== null) {
      if (/^\s*```\s*$/.test(line)) {
        html.push(`<pre class="code"><code>${esc(code.join("\n"))}</code></pre>`);
        code = null;
      } else {
        code.push(line);
      }
      continue;
    }
    if (/^\s*```(?:[\w.+-]+)?\s*$/.test(line)) {
      flushText();
      code = [];
      continue;
    }
    if (!line.trim()) { flushText(); continue; }

    const headerCells = markdownTableCells(line);
    const dividerCells = markdownTableCells(lines[lineIndex + 1] || "");
    const isTable = headerCells.length > 0
      && dividerCells.length === headerCells.length
      && dividerCells.every(cell => /^:?-{3,}:?$/.test(cell));
    if (isTable) {
      flushText();
      const alignments = dividerCells.map(cell => cell.startsWith(":") && cell.endsWith(":")
        ? "center" : cell.endsWith(":") ? "right" : "left");
      const rows = [];
      lineIndex += 2;
      while (lineIndex < lines.length) {
        const cells = markdownTableCells(lines[lineIndex]);
        if (!cells.length) break;
        rows.push(cells);
        lineIndex += 1;
      }
      lineIndex -= 1;
      const header = headerCells.map((cell, index) => `<th style="text-align:${alignments[index]}">${renderMarkdownInline(cell)}</th>`).join("");
      const body = rows.map(cells => `<tr>${headerCells.map((_, index) => `<td style="text-align:${alignments[index]}">${renderMarkdownInline(cells[index] || "")}</td>`).join("")}</tr>`).join("");
      html.push(`<div class="tablewrap"><table><thead><tr>${header}</tr></thead><tbody>${body}</tbody></table></div>`);
      continue;
    }

    const heading = line.match(/^\s{0,3}(#{1,3})\s+(.+)$/);
    if (heading) {
      flushText();
      const level = heading[1].length;
      html.push(`<h${level}>${renderMarkdownInline(heading[2])}</h${level}>`);
      continue;
    }
    if (/^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/.test(line)) {
      flushText();
      html.push("<hr>");
      continue;
    }
    const quoted = line.match(/^\s{0,3}>\s?(.*)$/);
    if (quoted) {
      flushParagraph(); flushList();
      quote.push(quoted[1]);
      continue;
    }
    const unordered = line.match(/^\s*[-+*]\s+(.+)$/);
    const ordered = line.match(/^\s*\d+[.)]\s+(.+)$/);
    if (unordered || ordered) {
      flushParagraph(); flushQuote();
      const nextType = unordered ? "ul" : "ol";
      if (listType && listType !== nextType) flushList();
      listType = nextType;
      listItems.push((unordered || ordered)[1]);
      continue;
    }
    flushList(); flushQuote();
    paragraph.push(line.trim());
  }
  if (code !== null) html.push(`<pre class="code"><code>${esc(code.join("\n"))}</code></pre>`);
  flushText();
  return html.join("");
}

// Chat uses the same deliberately small, safe Markdown subset as Notes. After
// escaping and rendering Markdown, mention styling is applied only to text
// nodes, never inside links or code.
function renderChatMarkdown(text) {
  const template = document.createElement("template");
  template.innerHTML = renderMarkdown(text);
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
  const textNodes = [];
  while (walker.nextNode()) {
    const parent = walker.currentNode.parentElement;
    if (!parent?.closest("code, pre, a")) textNodes.push(walker.currentNode);
  }
  for (const node of textNodes) {
    const parts = node.nodeValue.split(/(@[\w-]+)/g);
    if (parts.length === 1) continue;
    const fragment = document.createDocumentFragment();
    for (const part of parts) {
      if (/^@[\w-]+$/.test(part)) {
        const mention = document.createElement("span");
        mention.className = "mention" + (part.toLowerCase() === "@lead" ? " leadkeyword" : "");
        mention.textContent = part;
        fragment.appendChild(mention);
      } else {
        fragment.appendChild(document.createTextNode(part));
      }
    }
    node.replaceWith(fragment);
  }
  return template.innerHTML;
}

function regexEsc(text) {
  return String(text).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function addMsg(m) {
  if (m.kind !== "reminder") maybeDayChip(m.ts);
  if (m.id) {
    if (state.seen.has(m.id)) return;
    state.seen.add(m.id); if (m.id > state.lastId) state.lastId = m.id;
    msgById.set(m.id, m);
    state.msgs.push(m);          // keep the transcript so we can re-render
    if (state.msgs.length > 400) state.msgs.shift();
  }
  const mine = m.sender === state.name;
  const senderIsLead = state.settings?.lead === m.sender;
  const mentionsMe = new RegExp(`(^|\\s)@(${regexEsc(state.name)}|all)\\b`, "i").test(m.text) || m.target === state.name || m.target === "all";
  const row = document.createElement("div");
  row.className = "row" + (mine ? " mine" : "") + (mentionsMe && !mine ? " mentioned" : "")
    + (m.kind === "announce" ? " announce" : "")
    + (m.kind === "reminder" ? " locareminder" : "");
  if (m.id) row.dataset.id = m.id;
  row.dataset.ts = String(Number(m.ts || 0));
  row.dataset.text = (m.sender + " " + m.text).toLowerCase();

  // glyphs: * agent · . human — the table shorthand
  const glyph = m.sender_type === "agent" ? "*" : ".";
  let quote = "";
  if (m.reply_to && msgById.has(m.reply_to)) {
    const q = msgById.get(m.reply_to);
    quote = `<div class="quote" data-goto="${m.reply_to}">↩ ${esc(q.sender)}: ${esc(q.text.slice(0, 70))}</div>`;
  }
  let source = m.text;
  if (m.target && m.target !== "all" && !m.text.includes("@" + m.target)) {
    source = `@${m.target} ${source}`;
  }
  const txt = renderChatMarkdown(source);
  const time = m.ts ? fmtTime(m.ts) : "";
  const acts = m.id
    ? `<div class="lineacts" role="group" aria-label="Message actions">
        ${mine ? "" : `<button type="button" data-reactpick="${m.id}" aria-label="React to ${esc(m.sender)}">♡ react</button>`}
        <button type="button" data-reply="${m.id}" aria-label="Reply to ${esc(m.sender)}">↩ reply</button>
        <button type="button" data-mktask="${m.id}" aria-label="Make a task from ${esc(m.sender)}'s message">→ task</button>
      </div>`
    : "";
  row.innerHTML = `
    <div class="bubble${mentionsMe && !mine ? " mentioned" : ""}">
      <div class="line">
        <span class="glyph ${m.sender_type}">${glyph}</span>
        <span class="sender ${m.sender_type}">${esc(m.sender)}</span>${senderIsLead ? `<span class="leadtag">lead</span>` : ""}
        <span class="time" title="${esc(fmtFull(m.ts))}">${time}</span>
      </div>${quote}
      <div class="body markdown chatmarkdown">${txt}</div>
      ${m.attachments?.length ? `<div class="attachments" data-attfor></div>` : ""}
      ${m.id ? reactionHtml(m.id) : ""}
    </div>${acts}
    ${m.id && !mine ? `<div class="reactionpicker hidden" data-picker="${m.id}">${locaReactionSet.map(emoji => `<button type="button" data-react="${m.id}" data-emoji="${emoji}">${emoji}</button>`).join("")}</div>` : ""}`;
  if (m.kind === "reminder") {
    const reminderAt = Number(m.ts || 0);
    const newer = Array.from($("feed").querySelectorAll(".row[data-ts]"))
      .find(existing => Number(existing.dataset.ts || 0) > reminderAt);
    let anchor = newer;
    if (anchor?.previousElementSibling?.classList.contains("daychip")) {
      anchor = anchor.previousElementSibling;
    }
    $("feed").insertBefore(row, anchor || null);
  } else {
    $("feed").appendChild(row);
  }

  if (m.attachments?.length) renderAttachments(row, m);

  // @mention notification: flash the tab title if not focused.
  if (mentionsMe && !mine && document.hidden) flashTitle(m.sender);
}

/* ---- attachment rendering ---- */
// The GET endpoint is membership-gated, so we cannot point <img src> / <a href>
// straight at it (a browser element sends no auth header). We fetch the blob
// WITH the session header and hand the element a same-origin object URL instead.
function attachmentBlobUrl(room, id) {
  return `${serverBase()}/rooms/${encodeURIComponent(room)}/attachments/${encodeURIComponent(id)}`;
}
async function fetchAttachmentBlob(room, id) {
  const r = await fetch(attachmentBlobUrl(room, id), { headers: adminHeaders({}) });
  if (!r.ok) throw new Error(`attachment ${r.status}`);
  return URL.createObjectURL(await r.blob());
}
function attachmentChip(room, a) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "attachchip";
  btn.innerHTML = `<span class="aglyph">📎</span><span class="aname">${esc(a.name || a.id.slice(0, 8))}</span><span class="asize">${fmtBytes(a.size)}</span>`;
  btn.title = `${esc(a.mime || "")}`;
  btn.addEventListener("click", async () => {
    try {
      const url = await fetchAttachmentBlob(room, a.id);
      window.open(url, "_blank", "noopener");
    } catch (e) {
      addSys(`could not open ${a.name || "attachment"}`);
    }
  });
  return btn;
}
function renderAttachments(row, m) {
  const wrap = row.querySelector("[data-attfor]");
  if (!wrap) return;
  const room = m.room || state.room;
  for (const a of m.attachments) {
    const isImage = (a.mime || "").startsWith("image/");
    if (isImage) {
      const img = document.createElement("img");
      img.className = "attachimg";
      img.alt = a.name || "image";
      img.loading = "lazy";
      wrap.appendChild(img);
      fetchAttachmentBlob(room, a.id)
        .then((url) => {
          img.src = url;
          img.addEventListener("click", () => window.open(url, "_blank", "noopener"));
        })
        .catch(() => img.replaceWith(attachmentChip(room, a)));
    } else {
      wrap.appendChild(attachmentChip(room, a));
    }
  }
}

/* ---- @mention tab flash ---- */
let titleFlash = null;
const baseTitle = "loca";
function flashTitle(from) {
  if (titleFlash) return;
  let on = false;
  titleFlash = setInterval(() => { document.title = (on = !on) ? `💬 ${from} mentioned you` : baseTitle; }, 900);
  const stop = () => { clearInterval(titleFlash); titleFlash = null; document.title = baseTitle; document.removeEventListener("visibilitychange", onVis); };
  const onVis = () => { if (!document.hidden) stop(); };
  document.addEventListener("visibilitychange", onVis);
}
function addSys(t) { const d = document.createElement("div"); d.className = "sysline"; d.textContent = t; $("feed").appendChild(d); }
// WhatsApp-style: keep pinned to the bottom. If the user has scrolled up to
// read history, don't yank them down — but always land at the bottom on load.
// Re-render the whole feed from the kept transcript (used when the feed was
// cleared while we were on another tab).
function repaintFeed() {
  const msgs = state.msgs.slice();
  $("feed").innerHTML = "";
  lastDayKey = null;
  state.seen = new Set();
  state.msgs = [];
  for (const m of msgs) addMsg(m);
  rebuildReminderChatProjection();
}
function nearBottom() { const f = $("feed"); return f.scrollHeight - f.scrollTop - f.clientHeight < 200; }
function toBottom() { const f = $("feed"); f.scrollTop = f.scrollHeight; }
function scrollFeed(force) {
  if (!(force || nearBottom())) return;
  // Forced scrolls (room open / own message) must land at the very bottom even
  // after late layout (emoji, wrapping), so nudge across a few frames.
  toBottom();
  requestAnimationFrame(toBottom);
  if (force) { setTimeout(toBottom, 60); setTimeout(toBottom, 250); }
}
function esc(s) { return String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }

/* ---- chat mode ---- */
