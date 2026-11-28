const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

// A mentat room is founded as one from the create panel, may wait for its
// mentat, and has no seat for a rank: the Master watches from the door and
// writes only in its own turn in the line.
test("a mentat room: founded from the create panel, waits for its mentat, the Master asks in turn", async ({ browser, request }) => {
  const room = `mentat-${Date.now().toString(36)}`;
  expect((await request.post("/members", { headers: MASTER, data: { name: "sage", kind: "agent" } })).status()).toBeLessThan(500);

  const page = await masterPage(browser, request);
  await page.evaluate(() => openCreateLoca());
  await page.locator("#locaPreset input[value=mentat]").check();
  await expect(page.locator("#mentatCreateFields")).toBeVisible();
  await expect(page.locator("#extCreateExtensions")).toBeHidden();
  await expect(page.locator("#extCreateSubmit")).toHaveText("Create mentat room");
  await page.locator("#extCreateName").fill(room);
  // A topic says what the expert knows.
  await page.locator("#mentatTopic").fill("short");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreateError")).toContainText("at least 10 characters");
  await page.locator("#mentatTopic").fill("sessions, keys and the release order");
  // The mentat is chosen later.
  await expect(page.locator("#mentatPick")).toHaveValue("");
  await page.locator("#extCreateSubmit").click();

  await expect.poll(() => page.evaluate(() => state.room)).toBe(room);
  await expect(page.locator("#mentatBar")).toContainText("This room has no mentat yet");
  // No Lead, no seat for a rank, no Operator.
  await expect(page.locator("#leadSelect")).toBeHidden();
  await expect(page.locator("#seatToggle")).toBeHidden();
  await expect(page.locator("#msg")).toBeDisabled();
  const view = await (await request.get(`/rooms/${room}/mentat`, { headers: MASTER })).json();
  expect(view.state.mentat.topic).toBe("sessions, keys and the release order");
  expect(view.state.mentat.mentat ?? null).toBeNull();
  const operators = await (await request.get(`/rooms/${room}/operators`, { headers: MASTER })).json();
  expect(operators.appointed ?? null).toBeNull();

  // The Master appoints the mentat from the header picker.
  await expect(page.locator("#mentatSelect")).toBeVisible();
  await expect(page.locator("#mentatSelect option[value=sage]")).toHaveCount(1);
  await page.locator("#mentatSelect").selectOption("sage");
  await expect(page.locator("#mentatBar")).toContainText("Mentat @sage");
  await expect(page.locator("#mentatBar")).toContainText("Nobody inside");

  // The Master asks like anyone; its turn comes at once and opens the composer.
  page.once("dialog", (dialog) => dialog.accept("how do sessions renew after a 401?"));
  await page.locator("#mentatAsk").click();
  await expect(page.locator("#mentatBar")).toContainText("Inside: @");
  await expect(page.locator("#mentatBar")).toContainText("s to start");
  await expect(page.locator("#mentatAsk")).toHaveCount(0);
  await expect(page.locator("#msg")).toBeEnabled();

  // It ends the consultation, saying it was answered; the composer closes.
  await page.locator("[data-mentat-end=yes]").click();
  await expect(page.locator("#mentatBar")).toContainText("Nobody inside");
  await expect(page.locator("#msg")).toBeDisabled();
  const history = await (await request.get(`/rooms/${room}/mentat/history`, { headers: MASTER })).json();
  expect(history.requests[0].answered).toBe("yes");

  // Taking the appointment back keeps the room, waiting again.
  await page.locator("#mentatSelect").selectOption("");
  await expect(page.locator("#mentatBar")).toContainText("This room has no mentat yet");
});
