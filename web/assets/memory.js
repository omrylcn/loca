"use strict";
// Archived, room-scoped loca memory, read-only. PRINCIPLES "Wiki editor":
// there is no Memory role or owner to assign; Loca Wiki is the only shared
// knowledge surface. The server refuses every write and owner assignment, so
// this panel offers none.
const MEMORY_SHORT_MAX = 4 * 1024;
const MEMORY_ENTRY_MAX = 8 * 1024;
const MEMORY_LONG_MAX = 64 * 1024;

function memoryBytes(text) { return new TextEncoder().encode(text || "").length; }
function memoryWhen(value) { return value ? fmtFull(value) : "never"; }
function memoryNeedsAttention() {
  return state.memoryStatus === "absent" || state.memoryStatus === "inconsistent" || state.memoryStatus === "unknown" || !!state.memory?.over_budget;
}

const MEMORY_STATUSES = new Set(["ready", "absent", "empty", "inconsistent"]);
let memoryRequestSequence = 0;

function applyMemorySnapshot(memory) {
  const incomingVersion = Number(memory?.version || 0);
  const currentVersion = Number(state.memory?.version || 0);
  if (incomingVersion < currentVersion) return false;
  state.memory = memory;
  return true;
}

function onMemoryFrame(frame) {
  if (frame.room && frame.room !== state.room) return;
  if (Number(frame.version || 0) < Number(state.memory?.version || 0)) return;
  const knownStatus = MEMORY_STATUSES.has(frame.status);
  state.memoryStatus = frame.status;
  if (!knownStatus) state.memoryStatus = "unknown";
  state.memoryError = knownStatus ? "" : `Unknown memory status from server: ${String(frame.status)}`;
  applyMemorySnapshot(frame.status === "absent" ? null : {
    room: frame.room,
    owner: frame.owner,
    short: frame.short,
    long: frame.long,
    short_updated_at: frame.short_updated_at,
    long_updated_at: frame.long_updated_at,
    over_budget: frame.over_budget,
    version: frame.version,
    long_truncated: frame.long_truncated,
    long_omitted_bytes: frame.long_omitted_bytes,
    long_omitted_entries: frame.long_omitted_entries,
    long_uninjectable_entries: frame.long_uninjectable_entries,
  });
  renderMemory();
  // Memory ownership is worn in the roster too. Repaint from this same
  // authoritative frame so a transfer removes the old badge immediately.
  renderMembers();
  if (state.tab === "people") renderPeople();
  // The frame is the authority for status and the immediate attention dot.
  // Refresh the unbounded HTTP resource and provenance list in the background
  // without replacing that authoritative status with a client-side guess.
  fetchMemory(true);
}

async function fetchMemory(preserveStatus = false) {
  const room = state.room;
  if (!room) return false;
  const sequence = ++memoryRequestSequence;
  const current = () => state.room === room && sequence === memoryRequestSequence;
  const gen = state.buildingGen;
  state.memoryLoading = true;
  if (state.tab === "memory") renderMemory();
  try {
    const base = `${serverBase()}/rooms/${encodeURIComponent(room)}/memory`;
    const memoryResponse = await fetch(base, { headers: adminHeaders({}) });
    if (!current()) return false;
    if (memoryResponse.status === 404) {
      if (Number(state.memory?.version || 0) > 0) return false;
      state.memory = null;
      state.memoryEntries = [];
      state.memoryStatus = "absent";
      state.memoryError = "";
      return true;
    }
    if (!memoryResponse.ok) throw new Error(await memoryResponse.text() || `HTTP ${memoryResponse.status}`);
    const memory = await memoryResponse.json();
    if (!current() || Number(memory.version || 0) < Number(state.memory?.version || 0)) return false;
    const entries = [];
    let afterId = 0;
    for (;;) {
      const entriesResponse = await fetch(`${base}/entries?after_id=${afterId}&limit=200`, { headers: adminHeaders({}) });
      if (!current()) return false;
      if (entriesResponse.status === 409) {
        if (applyMemorySnapshot(memory)) state.memoryEntries = [];
        state.memoryError = await entriesResponse.text();
        state.memoryStatus = "inconsistent";
        return false;
      }
      if (!entriesResponse.ok) throw new Error(await entriesResponse.text() || `HTTP ${entriesResponse.status}`);
      const page = await entriesResponse.json();
      entries.push(...(page.entries || []));
      if (page.next_after_id == null) break;
      if (page.next_after_id <= afterId) throw new Error("memory entries cursor did not advance");
      afterId = page.next_after_id;
    }
    if (!current()) return false;
    if (applyMemorySnapshot(memory)) state.memoryEntries = entries;
    if (!preserveStatus || ["loading", "error", "absent"].includes(state.memoryStatus)) {
      state.memoryStatus = memory.short || memory.long ? "ready" : "empty";
    }
    if (state.memoryStatus !== "unknown") state.memoryError = "";
    return true;
  } catch (error) {
    if (!current() || buildingStale(gen)) return false;
    if (!state.memory) state.memoryStatus = "error";
    if (!preserveStatus || state.memoryStatus !== "unknown") state.memoryError = String(error.message || error);
    return false;
  } finally {
    if (current()) {
      state.memoryLoading = false;
      renderMemory();
    }
  }
}

function renderMemory() {
  if (!$("memoryPanel")) return;
  const room = state.room || "—";
  $("memoryHeading").textContent = room;
  $("memoryDot").classList.toggle("on", memoryNeedsAttention());
  if (state.memoryLoading && !state.memory) {
    $("memoryOverview").innerHTML = `<div class="sysline">Loading ${esc(room)} memory…</div>`;
    $("memoryShort").textContent = "";
    $("memoryEntries").innerHTML = "";
    $("memoryHealth").innerHTML = "";
    return;
  }
  const memory = state.memory;
  const status = state.memoryStatus;
  const shortBytes = memoryBytes(memory?.short);
  const longBytes = memoryBytes(memory?.long);
  const archived = `<div class="sysline">Archived memory is read-only: nobody writes it or is assigned to it. Loca Wiki is the shared knowledge surface.</div>`;
  $("memoryOverview").innerHTML = `${archived}<div class="memorygrid">
    <div class="memorycard"><small>Status</small><b>${esc(status)}</b></div>
    <div class="memorycard"><small>Owner</small><b>${esc(memory?.owner || "none")}</b></div>
    <div class="memorycard"><small>Short updated</small><b>${esc(memoryWhen(memory?.short_updated_at))}</b></div>
    <div class="memorycard"><small>Long updated</small><b>${esc(memoryWhen(memory?.long_updated_at))}</b></div>
    <div class="memorycard"><small>Revision</small><b>${Number(memory?.version || 0)}</b></div>
  </div>`;
  $("memoryShort").textContent = memory?.short || (status === "absent" ? "This loca has no archived memory." : "No current state was written.");
  const entries = state.memoryEntries || [];
  $("memoryEntries").innerHTML = entries.length ? "" : `<div class="sysline">No durable decisions.</div>`;
  for (const entry of entries) {
    const row = document.createElement("div");
    row.className = "memoryentry";
    const body = document.createElement("div");
    body.textContent = entry.text;
    const meta = document.createElement("span");
    meta.className = "memorymeta";
    meta.textContent = `#${entry.id} · ${entry.decided_by || "unknown"} · ${memoryWhen(entry.decided_at)} · ${memoryBytes(entry.text)} bytes`;
    row.append(body, meta);
    $("memoryEntries").appendChild(row);
  }
  const warnings = [];
  if (status === "inconsistent") warnings.push("Long memory and provenance entries disagree.");
  if (memory?.over_budget) warnings.push("Long memory is over the 32 KiB warning threshold.");
  if (state.memoryError) warnings.push(state.memoryError);
  if (memory?.long_truncated) warnings.push(`${memory.long_omitted_entries || 0} decisions omitted from the automatic wake snapshot.`);
  $("memoryHealth").innerHTML = `<div class="memorygrid">
    <div class="memorycard"><small>Short budget</small><b>${shortBytes} / ${MEMORY_SHORT_MAX} bytes</b></div>
    <div class="memorycard"><small>Single decision limit</small><b>${MEMORY_ENTRY_MAX} bytes</b></div>
    <div class="memorycard"><small>Long budget</small><b>${longBytes} / ${MEMORY_LONG_MAX} bytes</b></div>
    <div class="memorycard"><small>Write access</small><b>read-only</b></div>
  </div>${warnings.map(w => `<div class="sysline memorywarn">${esc(w)}</div>`).join("")}`;
}
