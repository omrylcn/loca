const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

async function ownerRoom(page, request, suffix) {
  const room = `review-${suffix}`;
  await provisionRooms(request, [room]);
  const name = `owner-${suffix}`;
  const admin = { "x-admin-token": "MASTER" };
  expect((await request.post("/members", { headers: admin, data: { name, kind: "user" } })).ok()).toBeTruthy();
  const invite = await (await request.post(`/rooms/${room}/invites`, { headers: admin, data: { name } })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite.token }, data: { name, kind: "user", loca: room },
  })).json();
  const headers = { "x-session-token": session.session_token };
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(({ room, name, token }) => {
    state.name = name; state.session = token; state.authStatus = "authenticated";
    state.adminSession = false; state.roomToken = "";
    document.body.classList.remove("locked");
    joinRoom(room);
  }, { room, name, token: session.session_token });
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  // The seated socket's first frame is `memory` (this loca has none archived).
  await expect.poll(() => page.evaluate(() => state.memoryStatus)).toBe("absent");
  return { room, headers, admin };
}

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
