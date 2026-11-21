"use strict";
const reactionByMessage = new Map();
const locaReactionSet = ["✓", "✦", "!", "♥"];

function reactionKey(messageId, emoji) { return `${messageId}\u0000${emoji}`; }
// The chips a message wears, a dashed "+" after them that opens the same
// picker as the hover "♡ react" (on someone else's message only: one does not
// react to one's own), and a quiet line naming who reacted with each mark.
function reactionHtml(messageId) {
  const shown = locaReactionSet
    .map(emoji => [emoji, reactionByMessage.get(reactionKey(messageId, emoji)) || []])
    .filter(([, actors]) => actors.length);
  const marks = shown.map(([emoji, actors]) => {
    const mine = actors.includes(state.name);
    return `<button type="button" class="reactionchip${mine ? " mine" : ""}" data-react="${messageId}" data-emoji="${emoji}" aria-pressed="${mine}" title="${esc(actors.join(", "))}">${emoji}<span>${actors.length}</span></button>`;
  }).join("");
  const message = msgById.get(Number(messageId));
  const mayReact = !!message && message.sender !== state.name;
  const add = shown.length && mayReact
    ? `<button type="button" class="reactadd" data-reactadd="${messageId}" aria-label="Add a reaction" title="Add a reaction">+</button>`
    : "";
  const who = shown.length
    ? `<span class="reactwho">${shown.map(([emoji, actors]) => `${emoji} ${esc(actors.join(", "))}`).join(" · ")}</span>`
    : "";
  return `<div class="reactions" data-reactions="${messageId}">${marks}${add}${who}</div>`;
}
// Open (or close) a message's reaction picker, closing any other one. The
// hover "♡ react" and the "+" chip both come here.
function toggleReactionPicker(messageId) {
  const panel = document.querySelector(`[data-picker="${Number(messageId)}"]`);
  document.querySelectorAll(".reactionpicker").forEach(p => { if (p !== panel) p.classList.add("hidden"); });
  panel?.classList.toggle("hidden");
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

  // "task #N" opens that task (taskview.js).
  const renderPlain = (plain) => taskRefLinks(esc(plain))
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

// `breaks`: a line break the writer typed stays a line break, as in a chat;
// without it (Notes, Wiki, Vault) the lines of a paragraph join, as in
// Markdown.
function renderMarkdown(text, { breaks = false } = {}) {
  const lines = String(text || "").replace(/\r\n?/g, "\n").split("\n");
  const html = [];
  let paragraph = [];
  let listType = null;
  let listItems = [];
  let quote = [];
  let code = null;

  const flushParagraph = () => {
    if (paragraph.length) {
      const body = breaks ? paragraph.map(renderMarkdownInline).join("<br>") : renderMarkdownInline(paragraph.join(" "));
      html.push(`<p>${body}</p>`);
    }
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

// Chat uses the same deliberately small, safe Markdown subset as Notes, and
// keeps every line break the writer typed. After
// escaping and rendering Markdown, mention styling is applied only to text
// nodes, never inside links or code.
function renderChatMarkdown(text) {
  const template = document.createElement("template");
  template.innerHTML = renderMarkdown(text, { breaks: true });
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
  const textNodes = [];
  while (walker.nextNode()) {
    const parent = walker.currentNode.parentElement;
    if (!parent?.closest("code, pre, a")) textNodes.push(walker.currentNode);
  }
  const appendMentions = (fragment, text) => {
    for (const part of text.split(/(@[\w-]+)/g)) {
      if (!part) continue;
      if (/^@[\w-]+$/.test(part)) {
        const mention = document.createElement("span");
        mention.className = "mention" + (part.toLowerCase() === "@lead" ? " leadkeyword" : "");
        mention.textContent = part;
        fragment.appendChild(mention);
      } else {
        fragment.appendChild(document.createTextNode(part));
      }
    }
  };
  for (const node of textNodes) {
    const value = node.nodeValue;
    if (!/@[\w-]|https:\/\/github\.com\//.test(value)) continue;
    const fragment = document.createDocumentFragment();
    // A bare GitHub pull request, issue or commit address becomes a link
    // (its card is drawn under the message); everything else is as before.
    for (const piece of value.split(/(https:\/\/github\.com\/[^\s<>"'`]+)/g)) {
      const url = piece.replace(/[.,;:!?)\]]+$/, "");
      if (piece.startsWith("https://github.com/") && githubRef(url)) {
        const link = document.createElement("a");
        link.href = url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.textContent = url;
        fragment.appendChild(link);
        appendMentions(fragment, piece.slice(url.length));
      } else {
        appendMentions(fragment, piece);
      }
    }
    node.replaceWith(fragment);
  }
  return template.innerHTML;
}

/* ---- GitHub link cards (client-side only: nothing is fetched) ---- */
// https://github.com/<owner>/<repo>/(pull|issues)/<n> or /commit/<sha>, with
// any path, query or fragment after it. The card's own link is rebuilt from
// the parsed parts, so only those characters ever reach it.
const GITHUB_REF = /^https:\/\/github\.com\/([A-Za-z0-9][A-Za-z0-9-]{0,38})\/([A-Za-z0-9._-]{1,100})\/(?:(pull|issues)\/(\d{1,10})|commit\/([0-9a-fA-F]{7,40}))(?:[/?#]\S*)?$/;
function githubRef(url) {
  const m = GITHUB_REF.exec(String(url || ""));
  if (!m) return null;
  const kind = m[3] === "pull" ? "pull" : m[3] === "issues" ? "issue" : "commit";
  const sha = m[5] ? m[5].toLowerCase() : "";
  const path = kind === "commit" ? `commit/${sha}` : `${m[3]}/${m[4]}`;
  return { kind, owner: m[1], repo: m[2], number: m[4] || "", sha, href: `https://github.com/${m[1]}/${m[2]}/${path}` };
}
const GITHUB_CARD = {
  pull: ["Pull request", '<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><circle cx="6" cy="18" r="2.5"/><path d="M6 8.5v7M18 15.5V10a3 3 0 0 0-3-3h-4"/>'],
  issue: ["Issue", '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2"/>'],
  commit: ["Commit", '<circle cx="12" cy="12" r="3.5"/><path d="M3 12h5.5M15.5 12H21"/>'],
};
// The cards for the GitHub links in a rendered message body (at most four,
// each reference once), or "" when there are none.
function githubCardsHtml(body) {
  if (!body) return "";
  const seen = new Set();
  const cards = [];
  for (const a of body.querySelectorAll("a[href]")) {
    const ref = githubRef(a.getAttribute("href"));
    if (!ref || seen.has(ref.href)) continue;
    seen.add(ref.href);
    const [label, icon] = GITHUB_CARD[ref.kind];
    const which = ref.kind === "commit"
      ? `<span class="lcsha">${esc(ref.sha.slice(0, 7))}</span>`
      : `<b>#${esc(ref.number)}</b>`;
    cards.push(`<a class="linkcard" data-linkkind="${ref.kind}" href="${esc(ref.href)}" target="_blank" rel="noopener noreferrer" title="${esc(ref.href)}">` +
      `<svg class="lcicon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">${icon}</svg>` +
      `<span class="lckind">${label}</span><span class="lcrepo">${esc(ref.owner)}/${esc(ref.repo)}</span>${which}</a>`);
    if (cards.length === 4) break;
  }
  return cards.length ? `<div class="linkcards">${cards.join("")}</div>` : "";
}

function regexEsc(text) {
  return String(text).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/* ---- pinned message (per-user, per-room, localStorage — no server state) ---- */
// Keyed by server + principal + room, so one identity's pin in one loca never
// leaks into another loca or another identity on the same browser. This is a
// personal reference only: pinning is task-INDEPENDENT and touches no shared
// room state.
function pinStorageKey() {
  return `loca-pinned:${serverBase()}:${state.principalId || state.name || ""}:${state.room || ""}`;
}
function loadPinned() {
  try {
    state.pinned = JSON.parse(localStorage.getItem(pinStorageKey()) || "null");
  } catch (e) {
    state.pinned = null;
  }
  state.pinnedExpanded = false;
  renderPinned();
}
function renderPinned() {
  const bar = $("pinnedBar");
  if (!bar) return;
  const pin = state.pinned;
  bar.classList.toggle("hidden", !pin || state.tab !== "chat");
  bar.classList.toggle("expanded", !!pin && state.pinnedExpanded);
  if (!pin) return;
  $("pinnedContent").setAttribute("aria-expanded", String(state.pinnedExpanded));
  $("pinnedContent").title = state.pinnedExpanded
    ? "collapse pinned message"
    : "expand pinned message";
  $("pinnedContent").innerHTML =
    `<div class="pinmeta">${esc(pin.sender)} · pinned · ${state.pinnedExpanded ? "collapse" : "expand"}</div>` +
    `<div class="pintext">${renderChatMarkdown(pin.text)}</div>`;
  document.querySelectorAll(".row[data-id]").forEach((row) => {
    row.classList.toggle("pinned-source", Number(row.dataset.id) === Number(pin.id));
  });
}
function togglePin(id) {
  const nid = Number(id);
  if (Number(state.pinned?.id) === nid) {
    // Unpin — must work even after a reload where the message is no longer in
    // the loaded feed (the bar renders from the stored snapshot, not msgById).
    state.pinned = null;
  } else {
    const m = msgById.get(nid);
    if (!m) return; // can only pin a message we currently hold
    state.pinned = { id: m.id, sender: m.sender, text: m.text, ts: m.ts };
  }
  state.pinnedExpanded = false;
  try {
    if (state.pinned) localStorage.setItem(pinStorageKey(), JSON.stringify(state.pinned));
    else localStorage.removeItem(pinStorageKey());
  } catch (e) {}
  renderPinned();
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
  const mentionsMe = new RegExp(`(^|\\s)@(${regexEsc(state.name)}|all)\\b`, "i").test(m.text) || m.target === state.name || m.target === "all";
  const row = document.createElement("div");
  row.className = "row" + (mine ? " mine" : "") + (mentionsMe && !mine ? " mentioned" : "")
    + (m.kind === "announce" ? " announce" : "")
    + (m.kind === "reminder" ? " locareminder" : "")
    + (Number(state.pinned?.id) === Number(m.id) ? " pinned-source" : "");
  if (m.id) row.dataset.id = m.id;
  // Who wrote it, for the row's edge (chatsurface.css): an agent, a person.
  row.dataset.kind = m.sender_type === "agent" ? "agent" : "person";
  row.dataset.ts = String(Number(m.ts || 0));
  row.dataset.text = (m.sender + " " + m.text).toLowerCase();

  // glyphs: * agent · . human — the table shorthand
  const glyph = m.sender_type === "agent" ? "*" : ".";
  let quote = "";
  if (m.reply_to) {
    // What it answers. On screen, the quote goes to it; further back than the
    // loaded chat, the quote opens the thread, which reads it (threads.js).
    const q = msgById.get(m.reply_to);
    const at = state.seen.has(m.reply_to) ? "data-goto" : "data-thread";
    quote = q
      ? `<div class="quote" ${at}="${Number(m.reply_to)}">↩ ${esc(q.sender)}: ${esc(q.text.slice(0, 70))}</div>`
      : `<div class="quote earlier" data-thread="${Number(m.reply_to)}">↩ replying to an earlier message</div>`;
  }
  let source = m.text;
  // In a direct message every word is to the other side; saying so on each line is noise.
  const addressed = !!(m.target && m.target !== "all" && !isDm(m.room) && !m.text.includes("@" + m.target));
  if (addressed) {
    source = `@${m.target} ${source}`;
  }
  const txt = renderChatMarkdown(source);
  const time = m.ts ? fmtMessageTime(m.ts) : "";
  const acts = m.id
    ? `<div class="lineacts" role="group" aria-label="Message actions">
        ${mine ? "" : `<button type="button" data-reactpick="${m.id}" aria-label="React to ${esc(m.sender)}">♡ react</button>`}
        <button type="button" data-reply="${m.id}" aria-label="Reply to ${esc(m.sender)}">↩ reply</button>
        <button type="button" data-pin="${m.id}" aria-label="Pin ${esc(m.sender)}'s message for yourself">⌖ pin</button>
      </div>`
    : "";
  row.innerHTML = `
    <span class="face" data-writer="${esc(m.sender_principal || "")}" data-sender="${esc(m.sender)}" data-kind="${esc(m.sender_type || "")}" aria-hidden="true">${faceHtml(m.sender, m.sender_type, m.sender_principal)}</span>
    <div class="bubble${mentionsMe && !mine ? " mentioned" : ""}">
      <div class="line">
        <span class="glyph ${m.sender_type}">${glyph}</span>
        <span class="sendercard" data-writer="${esc(m.sender_principal || "")}">${senderCardHtml(m.sender_principal)}</span><span class="sender ${m.sender_type}">${esc(m.sender)}</span><span class="roletags" data-sender="${esc(m.sender)}" data-writer="${esc(m.sender_principal || "")}">${roleTagsFor(m.sender, m.sender_principal)}</span>
        <span class="time" title="${esc(fmtFull(m.ts))}">${time}</span>
      </div>${quote}
      <div class="body markdown chatmarkdown">${txt}</div>
      ${m.attachments?.length ? `<div class="attachments" data-attfor></div>` : ""}
      ${m.id ? reactionHtml(m.id) : ""}
    </div>${acts}
    ${m.id && !mine ? `<div class="reactionpicker hidden" data-picker="${m.id}">${locaReactionSet.map(emoji => `<button type="button" data-react="${m.id}" data-emoji="${emoji}">${emoji}</button>`).join("")}</div>` : ""}`;
  // The target the client wrote in front of the words is the address, not
  // part of what was said: the redesign themes draw it as "→ @name".
  if (addressed) row.querySelector(".body .mention")?.classList.add("addr");
  // A GitHub pull request, issue or commit the words link to: a compact card
  // under them, built from the address alone (nothing is fetched).
  const body = row.querySelector(".body");
  const cards = githubCardsHtml(body);
  if (cards) body.insertAdjacentHTML("afterend", cards);
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

  // A freshly-rendered row that IS the pinned message must show the pinned-source
  // highlight (and keep the pinned bar current).
  if (Number(state.pinned?.id) === Number(m.id)) renderPinned();

  // A reply counts under the message it answers, and joins its open thread.
  if (m.id && typeof threadsNoteMessage === "function") threadsNoteMessage(m);

  // @mention notification: flash the tab title if not focused.
  if (mentionsMe && !mine && document.hidden) flashTitle(m.sender);
}

/* ---- attachment rendering ---- */
// The GET endpoint is membership-gated, so we cannot point <img src> / <a href>
// straight at it (a browser element sends no auth header). We fetch the blob
// WITH the session header and hand the element a same-origin object URL instead.
// Inline-image object URLs currently live in the feed. Tracked so they are
// revoked on feed repaint / room change / manual clear rather than leaking for
// the life of the process — which matters most for the long-lived Desktop shell.
const inlineAttachmentUrls = new Set();
function revokeInlineAttachmentUrls() {
  for (const url of inlineAttachmentUrls) URL.revokeObjectURL(url);
  inlineAttachmentUrls.clear();
}
function attachmentBlobUrl(room, id) {
  return `${serverBase()}/rooms/${encodeURIComponent(room)}/attachments/${encodeURIComponent(id)}`;
}
// The GET is membership-gated, so an <img src>/<a href> (which can't carry the
// session header) won't work; fetch WITH the header and hand back a same-origin
// object URL. The CALLER owns revoking it.
async function fetchAttachmentBlob(room, id) {
  const r = await fetch(attachmentBlobUrl(room, id), { headers: adminHeaders({}) });
  if (!r.ok) throw new Error(`attachment ${r.status}`);
  return URL.createObjectURL(await r.blob());
}
// Save an attachment as a file: fetched with the session, handed over as an
// object URL the page made, and that URL revoked once the save has had time.
async function saveAttachment(room, a) {
  const url = await fetchAttachmentBlob(room, a.id);
  const link = document.createElement("a");
  link.href = url;
  link.download = (a.name || "attachment").split(/[\\/]/).pop() || "attachment";
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}
// Open an attachment in a new tab, then revoke its URL after the tab has had
// time to load it, so an opened file is never held for the process lifetime.
// The desktop window opens no second window (main.rs), so there the file is
// saved instead and the shell says where it went.
function openAttachment(room, a) {
  const gen = state.buildingGen;
  if (window.__LOCA_DESKTOP__ === true) {
    saveAttachment(room, a)
      .catch(() => { if (!buildingStale(gen)) addSys(`could not save ${a.name || "attachment"}`); });
    return;
  }
  fetchAttachmentBlob(room, a.id)
    .then((url) => {
      window.open(url, "_blank", "noopener");
      setTimeout(() => URL.revokeObjectURL(url), 60000);
    })
    .catch(() => { if (!buildingStale(gen)) addSys(`could not open ${a.name || "attachment"}`); });
}
function attachmentChip(room, a) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "attachchip";
  // The file's kind on its tile (PDF, PNG…), as the redesign draws a file.
  const ext = (/\.([a-z0-9]{1,4})$/i.exec(a.name || "")?.[1] || "file").toLowerCase();
  btn.dataset.ext = ext;
  btn.innerHTML = `<span class="aglyph" aria-hidden="true">${esc(ext.toUpperCase())}</span><span class="aname">${esc(a.name || a.id.slice(0, 8))}</span><span class="asize">${fmtBytes(a.size)}</span><span>↓ Download</span>`;
  btn.title = `${esc(a.mime || "")}`;
  btn.setAttribute("aria-label", `Download ${a.name || "attachment"}`);
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    const gen = state.buildingGen;
    try {
      await saveAttachment(room, a);
    } catch (_) {
      if (!buildingStale(gen)) addSys(`could not download ${a.name || "attachment"}`);
    } finally {
      btn.disabled = false;
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
      wrap.appendChild(attachmentChip(room, a));
      fetchAttachmentBlob(room, a.id)
        .then((url) => {
          // The row may have been cleared (repaint/room change) while the blob
          // was in flight: free immediately and don't track a detached image.
          if (!img.isConnected) {
            URL.revokeObjectURL(url);
            return;
          }
          inlineAttachmentUrls.add(url);
          // Once decoded, the browser keeps the bitmap, so free the blob as soon
          // as it loads; the repaint/room-change sweep is the backstop for the
          // case where it never loads.
          const free = () => {
            URL.revokeObjectURL(url);
            inlineAttachmentUrls.delete(url);
          };
          img.addEventListener("load", free, { once: true });
          img.addEventListener("error", free, { once: true });
          img.src = url;
          // Clicking opens the full image; openAttachment re-fetches so the
          // inline blob is never kept alive for the popup.
          img.addEventListener("click", () => openAttachment(room, a));
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
  revokeInlineAttachmentUrls();
  $("feed").innerHTML = "";
  lastDayKey = null;
  state.seen = new Set();
  state.msgs = [];
  for (const m of msgs) addMsg(m);
  rebuildReminderChatProjection();
  renderPinned();
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
// The titles a sender wears at this table, shown on every line they write:
// Building rank, operator and lead. Operator is a seat a person is appointed
// to here; the Master holds its authority everywhere by rank, not by
// appointment (PRINCIPLES), so the Master's lines wear "Master", and
// "operator" only where the Master is the one appointed. A direct message has none.
//
// A rank is tied to the writer the server recorded on the line
// (`sender_principal`), never to the name: a name can be worn by someone else
// later, and a line by whoever wore it before must not gain that person's
// rank or seat. A line nobody verified wears neither. The Master is the
// principal the operators answer names; a Smaster is known while it is seated
// here (the roster carries its principal and rank); the Operator is the
// principal appointed. Lead is a name in the loca's settings, and is matched
// as one.
function roleTagsFor(name, writer) {
  if (!name || isDm(state.room)) return "";
  const op = state.locaOperator;
  const rank = !writer ? undefined
    : writer === op?.building_master?.principal_id ? "master"
    : (state.members || []).find(m => m.principal_id === writer)?.role;
  const isOperator = !!writer && writer === op?.appointed?.principal_id;
  const isLead = state.settings?.lead === name;
  // The Building's caretaker (the server declares it: /health loca_agents)
  // wears its title in every loca: operator is a loca's own, caretaker is
  // the Building's. Its name is the server's and nobody else may take it;
  // a line nobody verified still wears nothing.
  const isCaretaker = !!writer && (state.locaAgents || []).includes(name);
  return rankBadge(rank)
    + (isCaretaker ? `<span class="caretakertag" title="the Building's caretaker">caretaker</span>` : "")
    + (isOperator ? `<span class="optag" title="appointed loca operator">operator</span>` : "")
    + (isLead ? `<span class="leadtag" title="loca lead">lead</span>` : "")
    + (typeof taskBadgeFor === "function" ? taskBadgeFor(name, writer) : "");
}

// The operator and lead of a loca arrive after its history is drawn (and can
// change while it is open): relabel the lines already on screen.
function refreshSenderTags() {
  for (const el of document.querySelectorAll("#feed .roletags[data-sender]")) {
    el.innerHTML = roleTagsFor(el.dataset.sender, el.dataset.writer || "");
  }
}

// A writer's icon and full name, before the @name on every line they wrote.
// Keyed by the writer the server recorded (`sender_principal`), never by the
// name: a line nobody verified shows its plain name, whoever wears it now.
function senderCardHtml(writer) {
  const card = writer && typeof personCardById === "function" ? personCardById(writer) : null;
  if (!card) return "";
  return avatarHtml(card, card.name, "xs") + (card.full_name ? `<bdi class="fullname">${esc(card.full_name)}</bdi>` : "");
}

// The face at the head of a message: the writer's own avatar when their card
// has one; otherwise an agent's * on a square and a person's initials on a
// circle. Display only, like the card (the @name is who someone is).
function faceHtml(sender, kind, writer) {
  const card = writer && typeof personCardById === "function" ? personCardById(writer) : null;
  if (cardHasAvatar(card)) return avatarHtml(card, sender, "md");
  if (kind === "agent") return `<span class="avatar av-md av-rounded face-agent">*</span>`;
  return `<span class="avatar av-md av-circle face-person"><span class="avinitials">${esc(avatarInitials(card?.full_name, sender))}</span></span>`;
}

// The roster or a directory changed: redraw the cards already in the feed.
function refreshSenderCards() {
  for (const el of document.querySelectorAll("#feed .sendercard[data-writer]")) {
    el.innerHTML = senderCardHtml(el.dataset.writer);
  }
  for (const el of document.querySelectorAll("#feed .face[data-sender]")) {
    el.innerHTML = faceHtml(el.dataset.sender, el.dataset.kind, el.dataset.writer);
  }
}

function esc(s) { return String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }

/* ---- chat mode ---- */
