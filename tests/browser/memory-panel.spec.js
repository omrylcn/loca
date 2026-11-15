const { test, expect } = require("@playwright/test");

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
  await page.goto("/");
  await page.evaluate(() => {
    document.body.classList.remove("locked");
    document.querySelector(".main")?.classList.remove("global");
    document.querySelector(".main")?.style.setProperty("display", "flex", "important");
    state.room = "memory-test-room";
    state.name = "memory-owner";
  });
});

async function renderState(page, memoryStatus, memory) {
  await page.evaluate(({ memoryStatus, memory }) => {
    state.memoryStatus = memoryStatus;
    state.memory = memory;
    state.memoryEntries = [];
    state.memoryError = "";
    renderMemory();
  }, { memoryStatus, memory });
}

async function showMemoryPanel(page) {
  await page.evaluate(() => {
    state.tab = "memory";
    $("memoryPanel").classList.remove("hidden");
  });
}

async function renderPeopleWithMemory(page, { name = "memory-owner", owner = "memory-owner", admin = false } = {}) {
  await page.evaluate(({ name, owner, admin }) => {
    state.name = name;
    state.adminSession = admin;
    state.members = [
      { name: "memory-owner", type: "agent" },
      { name: "next-owner", type: "agent" },
      { name: "operator", type: "user" },
    ];
    state.seatedAway = [];
    state.memoryStatus = owner ? "ready" : "absent";
    state.memory = owner ? {
      room: state.room,
      owner,
      short: "",
      long: "",
      over_budget: false,
      version: 1,
    } : null;
    renderMemory();
    renderMembers();
  }, { name, owner, admin });
}

test("the Memory panel shows the loca name and separate short/long times", async ({ page }) => {
  await renderState(page, "ready", {
    room: "memory-test-room",
    owner: "memory-owner",
    short: "current state",
    long: "durable decision",
    short_updated_at: 1_700_000_000_000,
    long_updated_at: 1_700_000_100_000,
    over_budget: false,
    version: 7,
  });
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryPanel .memorytitle")).toContainText("memory-test-room");
  const overview = page.locator("#memoryOverview");
  await expect(overview).toContainText("Short updated");
  await expect(overview).toContainText("Long updated");
  await expect(page.locator("#memoryHealth")).toContainText("read-only");
});

test("archived memory offers no owner or write control, to its owner or an admin", async ({ page }) => {
  for (const who of [{ name: "memory-owner", admin: false }, { name: "operator", admin: true }]) {
    await renderPeopleWithMemory(page, { name: who.name, owner: "memory-owner", admin: who.admin });
    await showMemoryPanel(page);
    await expect(page.locator("#memoryOverview")).toContainText("Archived memory is read-only");
    await expect(page.locator("#memoryPanel")).not.toContainText("You are this loca's memory owner");
    for (const id of ["memoryOwnerEditor", "memoryShortEditor", "memoryDecisionEditor"]) {
      await expect(page.locator(`#${id}`)).toHaveCount(0);
    }
    await expect(page.locator("#memoryPanel button, #memoryPanel textarea, #memoryPanel select")).toHaveCount(0);
  }
});

for (const example of [
  { name: "over_budget", status: "ready", memory: { owner: "memory-owner", over_budget: true } },
  { name: "inconsistent", status: "inconsistent", memory: { owner: "memory-owner", over_budget: false } },
  { name: "absent", status: "absent", memory: null },
]) {
  test(`the Memory dot lights up in the ${example.name} state`, async ({ page }) => {
    await renderState(page, example.status, example.memory);
    await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  });
}

for (const status of ["ready", "empty"]) {
  test(`the Memory dot is off in the healthy ${status} state`, async ({ page }) => {
    await renderState(page, status, { owner: "memory-owner", short: "", long: "", over_budget: false });
    await expect(page.locator("#memoryDot")).not.toHaveClass(/\bon\b/);
  });
}

test("a WebSocket memory frame updates the authoritative status and the attention dot live", async ({ page }) => {
  await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      room: "memory-test-room", owner: "memory-owner", short: "live", long: "",
      short_updated_at: 10, long_updated_at: null, over_budget: true, version: 8,
    }),
  }));
  await page.route("**/rooms/memory-test-room/memory/entries**", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({ entries: [], next_after_id: null }),
  }));
  await page.evaluate(() => onFrame({
    t: "memory",
    room: "memory-test-room",
    owner: "memory-owner",
    status: "inconsistent",
    short: "live",
    long: "",
    short_updated_at: 10,
    long_updated_at: null,
    over_budget: true,
    version: 8,
    long_truncated: false,
    long_omitted_bytes: 0,
    long_omitted_entries: 0,
  }));
  await expect.poll(() => page.evaluate(() => state.memoryStatus)).toBe("inconsistent");
  await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  await expect.poll(() => page.evaluate(() => state.memory?.version)).toBe(8);
});

test("another loca's memory frame does not change the current panel", async ({ page }) => {
  await renderState(page, "ready", { owner: "memory-owner", short: "local", long: "", over_budget: false, version: 3 });
  await page.evaluate(() => onFrame({
    t: "memory", room: "other-room", owner: null, status: "absent", short: "", long: "",
    short_updated_at: null, long_updated_at: null, over_budget: false, version: 0,
  }));
  expect(await page.evaluate(() => ({ status: state.memoryStatus, version: state.memory.version })))
    .toEqual({ status: "ready", version: 3 });
});

test("an unknown server status is not silently dropped", async ({ page }) => {
  await page.evaluate(() => onFrame({
    t: "memory", room: "memory-test-room", owner: "memory-owner", status: "future-status",
    short: "", long: "", short_updated_at: null, long_updated_at: null,
    over_budget: false, version: 9,
  }));
  await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  await expect(page.locator("#memoryHealth")).toContainText("Unknown memory status");
  expect(await page.evaluate(() => state.memoryStatus)).toBe("unknown");
});

test("Archived memory metadata does not assign a live People role", async ({ page }) => {
  await renderPeopleWithMemory(page);

  const ownerSeat = page.locator("#onlineList .omem").filter({ hasText: "memory-owner" });
  const otherSeat = page.locator("#onlineList .omem").filter({ hasText: "next-owner" });
  await expect(ownerSeat).not.toContainText("memory owner");
  await expect(ownerSeat).not.toContainText("lead");
  await expect(otherSeat).not.toContainText("memory owner");
});

test("a loca without archived memory is not asked to assign an owner", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page, { owner: null });

  await expect(page.locator("#memoryPanel")).toContainText("This loca has no archived memory");
  await expect(page.locator("#memoryPanel")).not.toContainText("not assigned");
  await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
});

for (const order of ["response-then-frame", "frame-then-response"]) {
  test(`memory version does not roll back: ${order}`, async ({ page }) => {
    await renderState(page, "ready", { owner: "memory-owner", short: "v4", long: "", over_budget: false, version: 4 });
    const responseV5 = { room: "memory-test-room", owner: "memory-owner", short: "v5", long: "", over_budget: false, version: 5 };
    const frameV6 = { t: "memory", room: "memory-test-room", owner: "memory-owner", status: "ready", short: "v6", long: "", over_budget: false, version: 6 };
    await page.evaluate(({ order, responseV5, frameV6 }) => {
      if (order === "response-then-frame") {
        applyMemorySnapshot(responseV5);
        onMemoryFrame(frameV6);
      } else {
        onMemoryFrame(frameV6);
        applyMemorySnapshot(responseV5);
      }
    }, { order, responseV5, frameV6 });
    await expect.poll(() => page.evaluate(() => state.memory.version)).toBe(6);
    expect(await page.evaluate(() => state.memory.short)).toBe("v6");
  });
}
