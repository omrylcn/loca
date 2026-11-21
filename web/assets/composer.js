// The composer's writing tools: emoji, Markdown formatting and the keys for
// them. Everything here edits the words in #msg and nothing else: what the
// buttons write is the same Markdown chat already draws (chat.js
// renderMarkdown), so a message reads the same however it was written.
//
// Keys: Ctrl/Cmd+B bold, +I italic, +E code, +K link. Shift+Enter on a list
// or quote line carries the marker to the next line; on an empty item it
// ends the list. Enter still sends (app.js).

const COMPOSER_EMOJI = [
  ["Faces", "😀 😃 😄 😁 😆 😅 😂 🙂 😉 😊 😍 🥰 😘 😎 🤩 🤔 🤨 😐 😑 🙄 😏 😴 😌 😮 😯 😲 😳 🥺 😢 😭 😤 😡 🤯 😱 🤗 🫡 🤝 😇 🥳 🤓"],
  ["Hands", "👍 👎 👌 ✌️ 🤞 🤙 👋 👏 🙌 🙏 💪 👀 🫶 ✋ 👉 👈 👆 👇 ☝️ ✍️"],
  ["Work", "✅ ☑️ ❌ ⚠️ ❓ ❗ 💡 🔥 🚀 🎉 🎯 📌 📎 📝 📋 📊 📈 📉 🗂️ 🗓️ ⏰ ⏳ 🔒 🔑 🛠️ 🐛 🧪 📦 🔗 💬 📣 🔔"],
  ["Symbols", "❤️ 🧡 💛 💚 💙 💜 🖤 ⭐ ✨ ⚡ 💯 ➕ ➖ ✔️ ➡️ ⬅️ ⬆️ ⬇️ 🔁 🔄 🟢 🟡 🔴 🔵 ⚪ ⚫"],
];

// Put `text` in place of [start, end) the way typing would: the browser's
// own insert keeps Undo working; the fallback still tells the page the words
// changed.
function composerReplace(start, end, text, selectFrom = null, selectTo = null) {
  const t = $("msg");
  t.focus();
  t.setSelectionRange(start, end);
  let typed = false;
  try { typed = document.execCommand("insertText", false, text); } catch { typed = false; }
  if (!typed || t.value.slice(start, start + text.length) !== text) {
    t.setRangeText(text, start, end, "end");
    t.dispatchEvent(new Event("input", { bubbles: true }));
  }
  if (selectFrom !== null) t.setSelectionRange(start + selectFrom, start + (selectTo ?? selectFrom));
}

// **bold**, *italic*, `code`: around the selection, or around a placeholder
// left selected. Pressed again on wrapped words, it takes the marks off.
function composerWrap(mark, placeholder) {
  const t = $("msg");
  const { selectionStart: s, selectionEnd: e, value } = t;
  if (value.slice(s - mark.length, s) === mark && value.slice(e, e + mark.length) === mark && e > s) {
    composerReplace(s - mark.length, e + mark.length, value.slice(s, e), 0, e - s);
    return;
  }
  const words = value.slice(s, e) || placeholder;
  composerReplace(s, e, mark + words + mark, mark.length, mark.length + words.length);
}

// The whole lines the selection touches.
function composerLines() {
  const t = $("msg");
  const { selectionStart: s, selectionEnd: e, value } = t;
  const from = value.lastIndexOf("\n", s - 1) + 1;
  let to = value.indexOf("\n", e > s && value[e - 1] === "\n" ? e - 1 : e);
  if (to < 0) to = value.length;
  return { from, to, lines: value.slice(from, to).split("\n") };
}

const COMPOSER_LINE_MARK = {
  bullet: { has: /^\s*[-*+]\s/, strip: /^(\s*)[-*+]\s/, add: () => "- " },
  number: { has: /^\s*\d+[.)]\s/, strip: /^(\s*)\d+[.)]\s/, add: (i) => `${i + 1}. ` },
  quote: { has: /^\s*>\s?/, strip: /^(\s*)>\s?/, add: () => "> " },
};

// A list or a quote: every touched line gets the mark, or loses it when all
// of them already have it.
function composerLineMark(kind) {
  const mark = COMPOSER_LINE_MARK[kind];
  const { from, to, lines } = composerLines();
  const all = lines.every(l => mark.has.test(l));
  const next = all
    ? lines.map(l => l.replace(mark.strip, "$1"))
    : lines.map((l, i) => {
        const bare = Object.values(COMPOSER_LINE_MARK).reduce((acc, m) => acc.replace(m.strip, "$1"), l);
        return mark.add(i) + bare;
      });
  const text = next.join("\n");
  composerReplace(from, to, text, text.length);
}

function composerCodeBlock() {
  const t = $("msg");
  const { selectionStart: s, selectionEnd: e, value } = t;
  const before = s > 0 && value[s - 1] !== "\n" ? "\n" : "";
  const after = e < value.length && value[e] !== "\n" ? "\n" : "";
  const body = value.slice(s, e) || "code";
  const open = `${before}\`\`\`\n`;
  composerReplace(s, e, `${open}${body}\n\`\`\`${after}`, open.length, open.length + body.length);
}

// [words](https://…): with words selected the address is left selected to
// type over; with nothing selected the words are.
function composerLink() {
  const t = $("msg");
  const { selectionStart: s, selectionEnd: e, value } = t;
  const words = value.slice(s, e);
  if (words) {
    composerReplace(s, e, `[${words}](https://)`, words.length + 3, words.length + 11);
  } else {
    composerReplace(s, e, "[link](https://)", 1, 5);
  }
}

const COMPOSER_FORMATS = {
  bold: () => composerWrap("**", "bold"),
  italic: () => composerWrap("*", "italic"),
  code: () => composerWrap("`", "code"),
  codeblock: composerCodeBlock,
  bullet: () => composerLineMark("bullet"),
  number: () => composerLineMark("number"),
  quote: () => composerLineMark("quote"),
  link: composerLink,
};

// Shift+Enter on "- item", "3. item" or "> words" starts the next one; on a
// mark with nothing after it, the list ends there.
function composerContinueList(e) {
  const t = $("msg");
  if (t.selectionStart !== t.selectionEnd) return false;
  const caret = t.selectionStart;
  const from = t.value.lastIndexOf("\n", caret - 1) + 1;
  let to = t.value.indexOf("\n", caret);
  if (to < 0) to = t.value.length;
  const line = t.value.slice(from, to);
  const m = line.match(/^(\s*)([-*+]|\d+[.)]|>)(\s)(.*)$/);
  if (!m) return false;
  e.preventDefault();
  const [, indent, marker, , rest] = m;
  if (!rest.trim()) {
    composerReplace(from, to, indent, indent.length);
    return true;
  }
  const num = marker.match(/^(\d+)([.)])$/);
  const nextMark = num ? `${Number(num[1]) + 1}${num[2]}` : marker;
  const insert = `\n${indent}${nextMark} `;
  composerReplace(caret, caret, insert, insert.length);
  return true;
}

function composerClosePops(except) {
  for (const [btn, pop] of [["emojiBtn", "emojiPop"], ["formatBtn", "formatPop"]]) {
    if (pop === except) continue;
    $(pop)?.classList.add("hidden");
    $(btn)?.setAttribute("aria-expanded", "false");
  }
}

function composerTogglePop(btn, pop) {
  const open = $(pop).classList.contains("hidden");
  composerClosePops(pop);
  $(pop).classList.toggle("hidden", !open);
  $(btn).setAttribute("aria-expanded", String(open));
}

function renderEmojiPop() {
  $("emojiPop").innerHTML = COMPOSER_EMOJI.map(([group, list]) =>
    `<div class="emojigroup" role="group" aria-label="${esc(group)}"><div class="emojihead">${esc(group)}</div>`
    + `<div class="emojigrid">${list.split(" ").map(ch =>
      `<button type="button" class="emojichoice" data-emoji="${esc(ch)}" aria-label="${esc(ch)}">${esc(ch)}</button>`).join("")}</div></div>`
  ).join("");
}

(function wireComposerTools() {
  if (!$("msg") || !$("emojiPop") || !$("formatPop")) return;
  renderEmojiPop();
  // A tool button must not take the selection away from the words.
  for (const id of ["emojiBtn", "formatBtn", "emojiPop", "formatPop"]) {
    $(id).addEventListener("mousedown", (e) => { if (e.target.closest("button")) e.preventDefault(); });
  }
  $("emojiBtn").onclick = () => composerTogglePop("emojiBtn", "emojiPop");
  $("formatBtn").onclick = () => composerTogglePop("formatBtn", "formatPop");
  $("emojiPop").addEventListener("click", (e) => {
    const b = e.target.closest("[data-emoji]");
    if (!b) return;
    const t = $("msg");
    const ch = b.dataset.emoji;
    composerReplace(t.selectionStart, t.selectionEnd, ch, ch.length);
    composerClosePops();
  });
  $("formatPop").addEventListener("click", (e) => {
    const b = e.target.closest("[data-format]");
    if (b) COMPOSER_FORMATS[b.dataset.format]?.();
  });
  $("msg").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.shiftKey && !e.isComposing) { composerContinueList(e); return; }
    if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
    const key = { b: "bold", i: "italic", e: "code", k: "link" }[e.key.toLowerCase()];
    if (!key) return;
    e.preventDefault();
    COMPOSER_FORMATS[key]();
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest("#emojiPop, #formatPop, #emojiBtn, #formatBtn")) composerClosePops();
  });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") composerClosePops(); });
})();
