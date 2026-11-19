"use strict";
// The line above the message box: who a message will reach, said before it
// is sent. It shows the server's addressing as it stands (protocol.md:
// `target`, an @name in the words, `reply_to`); it decides nothing itself.
// Nobody named: everyone at the table reads it and nobody is asked to
// answer. @all asks every agent.

const ADDRESS_NAME = /(^|[^\w@.-])@([A-Za-z0-9][A-Za-z0-9_.-]*)/g;

function namedInWords(text) {
  const names = [];
  for (const match of String(text || "").matchAll(ADDRESS_NAME)) {
    const name = match[2].replace(/[.]+$/, "");
    if (name) names.push(name);
  }
  return names;
}

function addressees() {
  const known = new Map((state.members || []).map(m => [String(m.name).toLowerCase(), m.name]));
  known.set("all", "all");
  const named = [];
  const add = (name) => {
    const found = known.get(String(name).toLowerCase());
    if (found && found !== state.name && !named.includes(found)) named.push(found);
  };
  const target = $("target")?.value || "";
  if (target) add(target);
  for (const name of namedInWords($("msg")?.value)) add(name);
  const reply = state.replyTo != null && typeof msgById !== "undefined" ? msgById.get(Number(state.replyTo)) : null;
  if (reply?.sender) add(reply.sender);
  return named;
}

function renderAddressLine() {
  const line = $("addressLine");
  if (!line) return;
  const named = addressees();
  line.classList.toggle("addressed", named.length > 0);
  if (!named.length) {
    line.textContent = "To everyone · nobody is asked to answer";
  } else if (named.includes("all")) {
    line.textContent = "→ @all · every agent is asked";
  } else {
    line.textContent = "→ " + named.map(n => "@" + n).join(", ");
  }
}

$("msg").addEventListener("input", renderAddressLine);
$("target").addEventListener("change", renderAddressLine);
