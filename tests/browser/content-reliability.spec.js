const { test, expect } = require("@playwright/test");

async function ownerRoom(page, request, suffix) {
  const room = `review-${suffix}`;
  const name = `owner-${suffix}`;
  const admin = { "x-admin-token": "MASTER" };
  expect((await request.post("/members", { headers: admin, data: { name, kind: "user" } })).ok()).toBeTruthy();
  const invite = await (await request.post(`/rooms/${room}/invites`, { headers: admin, data: { name } })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite.token }, data: { name, kind: "user", loca: room },
  })).json();
  const headers = { "x-session-token": session.session_token };
  expect((await request.put(`/rooms/${room}/memory/owner`, { headers: admin, data: { owner: name } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/memory/short`, { headers, data: { text: "Persisted memory" } })).ok()).toBeTruthy();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(({ room, name, token }) => {
    state.name = name; state.session = token; state.authStatus = "authenticated";
    state.adminSession = false; state.roomToken = "";
    document.body.classList.remove("locked");
    joinRoom(room);
  }, { room, name, token: session.session_token });
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => state.memory?.short)).toBe("Persisted memory");
  return { room, headers, admin };
}

test("persisted memory opens without another WebSocket mutation", async ({ page, request }) => {
  await ownerRoom(page, request, "load");
  await page.evaluate(() => switchTab("memory")); // retained compatibility code, no navigation tab
  await expect(page.locator("#memoryShort")).toHaveText("Persisted memory");
  await expect(page.locator("#memoryShortEditor")).toBeVisible();
  await page.locator("#tabChat").click();
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryShortEditor")).toBeVisible();
});

test("live memory refresh preserves an unfocused dirty draft", async ({ page, request }) => {
  const { room, headers } = await ownerRoom(page, request, "draft");
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryShortEditor")).toBeVisible();
  await page.locator("#memoryShortInput").fill("UNSAVED MEMORY DRAFT");
  await page.locator("#memoryShortInput").press("Tab");
  expect((await request.put(`/rooms/${room}/memory/short`, { headers, data: { text: "Another writer" } })).ok()).toBeTruthy();
  await expect(page.locator("#memoryShort")).toHaveText("Another writer");
  await expect(page.locator("#memoryShortInput")).toHaveValue("UNSAVED MEMORY DRAFT");
});

test("accepted write with failed readback never claims verification", async ({ page, request }) => {
  const { room } = await ownerRoom(page, request, "verify");
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryShortEditor")).toBeVisible();
  await page.route(`**/rooms/${room}/memory`, route => route.fulfill({ status: 503, body: "readback unavailable" }));
  await page.locator("#memoryShortInput").fill("Accepted new value");
  await page.locator("#memorySaveShort").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("verification could not complete");
  await expect(page.locator("#memoryWriteResult")).not.toContainText("Saved and verified");
  await page.unroute(`**/rooms/${room}/memory`);
  await page.evaluate(() => fetchMemory(true));
  await expect.poll(() => page.evaluate(() => state.memoryError)).toBe("");
});

test("real decision append is verified against the server provenance list", async ({ page, request }) => {
  const { room } = await ownerRoom(page, request, "decision");
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryDecisionEditor")).toBeVisible();
  await page.locator("#memoryDecisionInput").fill("Decision with a real provenance record");
  await page.locator("#memoryAddDecision").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("Saved and verified");
  await expect(page.locator("#memoryEntries")).toContainText("Decision with a real provenance record");
  await expect(page.locator("#memoryDecisionInput")).toHaveValue("");
});

test("an older identical decision cannot verify a new append", async ({ page, request }) => {
  const { room, headers } = await ownerRoom(page, request, "decision-identity");
  const text = "PostgreSQL kullanacağız";
  const oldResponse = await request.post(`/rooms/${room}/memory/entries`, { headers, data: { text } });
  expect(oldResponse.status()).toBe(201);
  const older = await oldResponse.json();
  await page.evaluate(() => switchTab("memory"));
  await expect(page.locator("#memoryEntries")).toContainText(`#${older.id}`);
  let created;
  await page.route(`**/rooms/${room}/memory/entries**`, async route => {
    if (route.request().method() === "POST") {
      const response = await route.fetch();
      created = await response.json();
      return route.fulfill({ response });
    }
    return route.fulfill({ status: 200, contentType: "application/json",
      body: JSON.stringify({ entries: [older], next_after_id: null }) });
  });
  await page.locator("#memoryDecisionInput").fill(text);
  await page.locator("#memoryAddDecision").click();
  await expect(page.locator("#memoryWriteResult")).toContainText("verification could not complete");
  await expect(page.locator("#memoryWriteResult")).not.toContainText("Saved and verified");
  expect(created.id).not.toBe(older.id);
  const actual = await (await request.get(`/rooms/${room}/memory/entries`, { headers })).json();
  expect(actual.entries.filter(entry => entry.text === text).map(entry => entry.id)).toEqual([older.id, created.id]);
  await page.unroute(`**/rooms/${room}/memory/entries**`);
  await page.evaluate(() => fetchMemory(true));
  await expect(page.locator("#memoryEntries")).toContainText(`#${created.id}`);
});

test("switching note editors does not transfer the previous note draft", async ({ page, request }) => {
  const { room, headers } = await ownerRoom(page, request, "edit-switch");
  for (const key of ["first", "second"]) {
    expect((await request.post(`/rooms/${room}/notes`, { headers, data: { key, body: `${key} original`, by: "ignored" } })).ok()).toBeTruthy();
  }
  await page.locator("#tabNotes").click();
  await expect(page.locator('[data-edit="first"]')).toBeVisible();
  await page.locator('[data-edit="first"]').click();
  await page.locator("#ed-body").fill("first unsaved");
  await page.locator('[data-edit="second"]').click();
  await expect(page.locator("#ed-body")).toHaveValue("second original");
  await page.locator('[data-edit="first"]').click();
  await expect(page.locator("#ed-body")).toHaveValue("first unsaved");
});

test("archived room rejects note save without destroying the typed draft", async ({ page, request }) => {
  const { room, headers, admin } = await ownerRoom(page, request, "note");
  expect((await request.post(`/rooms/${room}/notes`, { headers, data: { key: "plan", title: "Plan", body: "original", by: "ignored" } })).ok()).toBeTruthy();
  await page.locator("#tabNotes").click();
  await page.locator('[data-edit="plan"]').click();
  await page.locator("#ed-body").fill("UNSAVED NOTE DRAFT");
  expect((await request.put(`/rooms/${room}/settings`, { headers: admin, data: { archived: true } })).ok()).toBeTruthy();
  await page.locator('[data-save="plan"]').click();
  await expect(page.locator("#notesError")).toContainText("read-only");
  await expect(page.locator("#ed-body")).toHaveValue("UNSAVED NOTE DRAFT");
});

for (const status of [401, 413, 503]) {
  test(`note create keeps its draft on HTTP ${status}`, async ({ page, request }) => {
    const { room } = await ownerRoom(page, request, `create-${status}`);
    await page.locator("#tabNotes").click();
    await page.locator("#nnKey").fill("new-note");
    await page.locator("#nnBody").fill("KEEP THIS DRAFT");
    await page.route(`**/rooms/${room}/notes`, route => route.request().method() === "POST"
      ? route.fulfill({ status, body: "injected write rejection" }) : route.continue());
    await page.locator("#nnCreate").click();
    await expect(page.locator("#notesError")).toContainText("injected write rejection");
    await expect(page.locator("#nnBody")).toHaveValue("KEEP THIS DRAFT");
  });
}
