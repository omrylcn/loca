"use strict";
// The agent kit version a connected agent runs: `kit_version` on a roster
// seat or a Building list row, sent by the kit when it connects (display
// only, never an authority; docs/protocol.md "Kit version"). Shown to the
// Master, a Smaster and the loca's Operator only, as "kit 0.15.0", with a
// plain mark when it is older than this server (its `/health` version).

const KIT_VERSION_SHAPE = /^(0|[1-9]\d{0,3})\.(0|[1-9]\d{0,3})\.(0|[1-9]\d{0,3})(?:-[0-9A-Za-z.-]{1,16})?$/;

function kitVersionCore(value) {
  const match = typeof value === "string" && value.length <= 32 ? KIT_VERSION_SHAPE.exec(value) : null;
  return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
}

// Older than the server only on MAJOR.MINOR.PATCH; anything unreadable is not.
function kitVersionOlder(kit, server) {
  const a = kitVersionCore(kit), b = kitVersionCore(server);
  if (!a || !b) return false;
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i];
  return false;
}

// Who sees it: on a loca's roster `seat`, the Master, a Smaster and that
// loca's Operator (isLocaOperator); on a `building` list row, the Master and
// a Smaster, the only ones who read that list.
function kitVersionViewer(where) {
  return where === "seat" ? isLocaOperator() : isAdmin();
}

// "kit X" (+ the older mark) for `entry` on a `where` row, worn as the row's
// own small tag class `cls`; "" for anyone else or without a version.
function kitVersionHtml(entry, where, cls = "otag") {
  const kit = entry?.kit_version;
  if (!kitVersionCore(kit) || !kitVersionViewer(where)) return "";
  const server = kitServerVersion();
  const older = kitVersionOlder(kit, server)
    ? ` <span class="${cls} kitold" title="This agent runs an older kit than this server; it keeps working but may lack newer commands.">older than this server (${esc(server)})</span>`
    : "";
  return ` <span class="${cls} kitver" title="Agent kit version, sent by the agent when it connected">kit ${esc(kit)}</span>${older}`;
}

// This server's own version (`/health`), asked once per server the first
// time a version is shown; what shows one is drawn again when it arrives.
let kitServer = { base: null, version: "" };
function kitServerVersion() {
  const base = serverBase();
  if (kitServer.base !== base) {
    kitServer = { base, version: "" };
    fetch(`${base}/health`).then(r => r.json()).then(h => {
      if (kitServer.base !== base || typeof h?.version !== "string") return;
      kitServer.version = h.version;
      const shown = list => Array.isArray(list) && list.some(p => p?.kit_version);
      if (state.room && shown(state.members)) renderMembers();
      if (shown(state.lobby)) renderLobby();
      if (state.tab === "people" && shown(state.people)) renderPeople();
    }).catch(() => { if (kitServer.base === base) kitServer.base = null; });
  }
  return kitServer.version;
}
