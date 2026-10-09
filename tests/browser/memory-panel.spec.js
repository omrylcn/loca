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

test("Memory paneli loca adini ve ayri short/long saatlerini gosterir", async ({ page }) => {
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
  await expect(page.locator("#memoryHealth")).toContainText("owner");
});

test("Building master memory owner secip sunucudan dogrular", async ({ page }) => {
  await page.route("**/rooms/memory-test-room/memory/owner", async route => {
    expect(route.request().method()).toBe("PUT");
    expect(route.request().postDataJSON()).toEqual({ owner: "next-owner" });
    await route.fulfill({ status: 204 });
  });
  await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({ room: "memory-test-room", owner: "next-owner", short: "", long: "", version: 2 }),
  }));
  await page.route("**/rooms/memory-test-room/memory/entries**", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({ entries: [], next_after_id: null }),
  }));
  await renderPeopleWithMemory(page, { name: "operator", owner: "memory-owner", admin: true });
  await showMemoryPanel(page);
  await expect(page.locator("#memoryOwnerEditor")).toBeVisible();
  await page.locator("#memoryOwnerSelect").selectOption("next-owner");
  await page.locator("#memorySaveOwner").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("changed and verified");
  await expect(page.locator("#memoryOverview")).toContainText("next-owner");
});

test("normal uye memory owner secicisini goremez", async ({ page }) => {
  await renderPeopleWithMemory(page, { name: "memory-owner", owner: "memory-owner", admin: false });
  await showMemoryPanel(page);
  await expect(page.locator("#memoryOwnerEditor")).toBeHidden();
});

for (const example of [
  { name: "over_budget", status: "ready", memory: { owner: "memory-owner", over_budget: true } },
  { name: "inconsistent", status: "inconsistent", memory: { owner: "memory-owner", over_budget: false } },
  { name: "absent", status: "absent", memory: null },
]) {
  test(`Memory noktasi ${example.name} durumunda yanar`, async ({ page }) => {
    await renderState(page, example.status, example.memory);
    await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  });
}

for (const status of ["ready", "empty"]) {
  test(`Memory noktasi saglikli ${status} durumunda sonuktur`, async ({ page }) => {
    await renderState(page, status, { owner: "memory-owner", short: "", long: "", over_budget: false });
    await expect(page.locator("#memoryDot")).not.toHaveClass(/\bon\b/);
  });
}

test("WebSocket memory karesi yetkili statusu ve dikkat noktasini canli gunceller", async ({ page }) => {
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

test("baska locanin memory karesi mevcut panoyu degistirmez", async ({ page }) => {
  await renderState(page, "ready", { owner: "memory-owner", short: "local", long: "", over_budget: false, version: 3 });
  await page.evaluate(() => onFrame({
    t: "memory", room: "other-room", owner: null, status: "absent", short: "", long: "",
    short_updated_at: null, long_updated_at: null, over_budget: false, version: 0,
  }));
  expect(await page.evaluate(() => ({ status: state.memoryStatus, version: state.memory.version })))
    .toEqual({ status: "ready", version: 3 });
});

test("bilinmeyen sunucu statusu sessizce atilmaz", async ({ page }) => {
  await page.evaluate(() => onFrame({
    t: "memory", room: "memory-test-room", owner: "memory-owner", status: "future-status",
    short: "", long: "", short_updated_at: null, long_updated_at: null,
    over_budget: false, version: 9,
  }));
  await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  await expect(page.locator("#memoryHealth")).toContainText("Unknown memory status");
  expect(await page.evaluate(() => state.memoryStatus)).toBe("unknown");
});

test("yazma kontrolleri yalniz memory owner icin gorunur", async ({ page }) => {
  await showMemoryPanel(page);
  await renderState(page, "ready", { owner: "someone-else", short: "", long: "", over_budget: false });
  await expect(page.locator("#memoryShortEditor")).toBeHidden();
  await expect(page.locator("#memoryDecisionEditor")).toBeHidden();
  await renderState(page, "ready", { owner: "memory-owner", short: "", long: "", over_budget: false });
  await expect(page.locator("#memoryShortEditor")).toBeVisible();
  await expect(page.locator("#memoryDecisionEditor")).toBeVisible();
});

test("admin olsa da owner olmayan kimlik yazamaz", async ({ page }) => {
  await page.evaluate(() => {
    state.adminToken = "present";
    state.name = "operator";
  });
  await renderState(page, "ready", { owner: "memory-owner", short: "", long: "", over_budget: false });
  expect(await page.evaluate(() => writeMemory("short", { method: "PUT" }, "blocked"))).toBe(false);
  await expect(page.locator("#memoryWriteResult")).toContainText("Only the memory owner");
});

test("People listesi memory owner unvanini lead rolunden ayri gosterir", async ({ page }) => {
  await renderPeopleWithMemory(page);

  const ownerSeat = page.locator("#onlineList .omem").filter({ hasText: "memory-owner" });
  const otherSeat = page.locator("#onlineList .omem").filter({ hasText: "next-owner" });
  await expect(ownerSeat).toContainText("memory owner");
  await expect(ownerSeat).not.toContainText("lead");
  await expect(otherSeat).not.toContainText("memory owner");
});

test("memory owner kendi sorumluluk bildirimini gorur", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page);

  await expect(page.locator("#memoryPanel")).toContainText("You are this loca's memory owner");
  await expect(page.locator("#memoryPanel")).toContainText("Keep the current state and durable decisions up to date");
});

test("memory owner devri eski sahibin unvanini ve yazma kontrollerini aninda kaldirir", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page);
  await expect(page.locator("#memoryShortEditor")).toBeVisible();

  await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      room: "memory-test-room", owner: "next-owner", short: "", long: "",
      short_updated_at: null, long_updated_at: null, over_budget: false, version: 2,
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
    owner: "next-owner",
    status: "ready",
    short: "",
    long: "",
    short_updated_at: null,
    long_updated_at: null,
    over_budget: false,
    version: 2,
  }));

  await expect(page.locator("#memoryShortEditor")).toBeHidden();
  await expect(page.locator("#memoryDecisionEditor")).toBeHidden();
  await expect(page.locator("#onlineList .omem").filter({ hasText: "memory-owner" }))
    .not.toContainText("memory owner");
  await expect(page.locator("#onlineList .omem").filter({ hasText: "next-owner" }))
    .toContainText("memory owner");
});

test("memory owner devri yeni sahibin yazma kontrollerini ve sorumluluk bildirimini aninda acar", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page, { name: "next-owner", owner: "memory-owner" });
  await expect(page.locator("#memoryShortEditor")).toBeHidden();

  await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      room: "memory-test-room", owner: "next-owner", short: "", long: "",
      short_updated_at: null, long_updated_at: null, over_budget: false, version: 2,
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
    owner: "next-owner",
    status: "ready",
    short: "",
    long: "",
    short_updated_at: null,
    long_updated_at: null,
    over_budget: false,
    version: 2,
  }));

  await expect(page.locator("#memoryShortEditor")).toBeVisible();
  await expect(page.locator("#memoryDecisionEditor")).toBeVisible();
  await expect(page.locator("#memoryPanel")).toContainText("You are this loca's memory owner");
});

test("sahipsiz loca Memory panelinde gorunur owner uyarisi verir", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page, { owner: null });

  await expect(page.locator("#memoryPanel")).toContainText("Memory owner not assigned");
  await expect(page.locator("#memoryDot")).toHaveClass(/\bon\b/);
  await expect(page.locator("#memoryShortEditor")).toBeHidden();
  await expect(page.locator("#memoryDecisionEditor")).toBeHidden();
});

test("admin olmak memory owner unvani veya yazma kontrolu kazandirmaz", async ({ page }) => {
  await showMemoryPanel(page);
  await renderPeopleWithMemory(page, { name: "operator", owner: "memory-owner", admin: true });

  const adminSeat = page.locator("#onlineList .omem").filter({ hasText: "operator" });
  await expect(adminSeat).not.toContainText("memory owner");
  await expect(page.locator("#memoryShortEditor")).toBeHidden();
  await expect(page.locator("#memoryDecisionEditor")).toBeHidden();
  expect(await page.evaluate(() => writeMemory("short", { method: "PUT" }, "blocked"))).toBe(false);
  await expect(page.locator("#memoryWriteResult")).toContainText("Only the memory owner");
});

for (const rejected of [
  { name: "kapali loca", status: 409, path: "short", method: "PUT", message: "this loca is closed — read-only" },
  { name: "short 4 KiB", status: 413, path: "short", method: "PUT", message: "short memory exceeds the 4 KiB limit" },
  { name: "entry 8 KiB", status: 413, path: "entries", method: "POST", message: "long-memory entry exceeds the 8 KiB wake-injection budget" },
  { name: "long 64 KiB", status: 413, path: "entries", method: "POST", message: "long memory is a finite append-only ledger (64 KiB); existing entries remain readable, but no further entries can be appended" },
]) {
  test(`${rejected.name} reddi sunucu mesaji ile gorunur`, async ({ page }) => {
    await page.route(`**/rooms/memory-test-room/memory/${rejected.path}`, route => route.fulfill({
      status: rejected.status, body: rejected.message,
    }));
    await renderState(page, "ready", {
      owner: "memory-owner", short: "unchanged", long: "", short_updated_at: 44,
      over_budget: false, version: 5,
    });
    expect(await page.evaluate(({ path, method }) => writeMemory(path, { method }, "attempt"), rejected)).toBe(false);
    await expect(page.locator("#memoryWriteResult")).toContainText(rejected.message);
    expect(await page.evaluate(() => state.memory.short_updated_at)).toBe(44);
    expect(await page.evaluate(() => state.memory.version)).toBe(5);
  });
}

test("owner short yazimini dogru uca yollar ve sunucudan yeniden okur", async ({ page }) => {
  let puts = 0;
  let reads = 0;
  await page.route("**/rooms/memory-test-room/memory/short", async route => {
    puts += 1;
    expect(route.request().method()).toBe("PUT");
    expect((await route.request().postDataJSON()).text).toBe("new current state");
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  await page.route("**/rooms/memory-test-room/memory", route => {
    reads += 1;
    return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({
      room: "memory-test-room", owner: "memory-owner", short: "new current state", long: "",
      short_updated_at: 20, long_updated_at: null, over_budget: false, version: 10,
    }) });
  });
  await page.route("**/rooms/memory-test-room/memory/entries**", route => route.fulfill({
    status: 200, contentType: "application/json", body: JSON.stringify({ entries: [], next_after_id: null }),
  }));
  await renderState(page, "ready", { owner: "memory-owner", short: "old", long: "", over_budget: false, version: 9 });
  await showMemoryPanel(page);
  await page.locator("#memoryShortInput").fill("new current state");
  await page.locator("#memorySaveShort").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("verified from the server");
  expect(puts).toBe(1);
  expect(reads).toBeGreaterThan(0);
  expect(await page.evaluate(() => state.memory.version)).toBe(10);
});

test("owner karari POST entries ucuna yollar", async ({ page }) => {
  let posts = 0;
  await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
    status: 200, contentType: "application/json", body: JSON.stringify({
      room: "memory-test-room", owner: "memory-owner", short: "", long: "durable choice",
      short_updated_at: null, long_updated_at: 30, over_budget: false, version: 4,
    }),
  }));
  await page.route("**/rooms/memory-test-room/memory/entries**", async route => {
    if (route.request().method() === "POST") {
      posts += 1;
      expect((await route.request().postDataJSON()).text).toBe("durable choice");
      return route.fulfill({ status: 201, contentType: "application/json", body: JSON.stringify({
        id: 1, room: "memory-test-room", text: "durable choice", decided_by: "memory-owner", decided_at: 30,
      }) });
    }
    return route.fulfill({
      status: 200, contentType: "application/json", body: JSON.stringify({ entries: [{ id: 1, text: "durable choice", decided_by: "memory-owner", decided_at: 30 }], next_after_id: null }),
    });
  });
  await renderState(page, "ready", { owner: "memory-owner", short: "", long: "", over_budget: false, version: 3 });
  await showMemoryPanel(page);
  await page.locator("#memoryDecisionInput").fill("durable choice");
  await page.locator("#memoryAddDecision").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("verified from the server");
  expect(posts).toBe(1);
});

for (const receipt of [
  { name: "missing ID", body: JSON.stringify({ room: "memory-test-room", text: "repeat" }) },
  { name: "malformed JSON", body: "not JSON" },
  { name: "unsafe ID", readbackId: Number.MAX_SAFE_INTEGER + 1, body: JSON.stringify({ id: Number.MAX_SAFE_INTEGER + 1, room: "memory-test-room", text: "repeat" }) },
]) {
  test(`accepted decision with ${receipt.name} cannot claim verification`, async ({ page }) => {
    await page.route("**/rooms/memory-test-room/memory", route => route.fulfill({
      status: 200, contentType: "application/json", body: JSON.stringify({
        room: "memory-test-room", owner: "memory-owner", short: "", long: "repeat", version: 4,
      }),
    }));
    await page.route("**/rooms/memory-test-room/memory/entries**", route => route.fulfill({
      status: route.request().method() === "POST" ? 201 : 200,
      contentType: "application/json",
      body: route.request().method() === "POST" ? receipt.body : JSON.stringify({
        entries: [{ id: receipt.readbackId ?? 1, text: "repeat", decided_by: "memory-owner", decided_at: 30 }], next_after_id: null,
      }),
    }));
    await renderState(page, "ready", { owner: "memory-owner", short: "", long: "", version: 3 });
    await showMemoryPanel(page);
    await page.locator("#memoryDecisionInput").fill("repeat");
    await page.locator("#memoryAddDecision").click();
    await expect(page.locator("#memoryWriteResult")).toContainText("Saved by the server; verification could not complete");
    await expect(page.locator("#memoryWriteResult")).not.toContainText("Saved and verified");
  });
}

for (const order of ["response-then-frame", "frame-then-response"]) {
  test(`memory version geri sarmaz: ${order}`, async ({ page }) => {
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
