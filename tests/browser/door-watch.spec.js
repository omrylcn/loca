const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const MASTER = { "x-admin-token": "MASTER" };

async function masterSession(request, name) {
  const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
  const { pairing_code } = await paired.json();
  const signedIn = await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name, kind: "user" },
  });
  expect(signedIn.ok()).toBe(true);
  return signedIn.json();
}

async function openAs(page, info, name, seatedRooms) {
  await page.addInitScript(({ token, expiresAt, name, seatedRooms }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
    if (seatedRooms && !sessionStorage.getItem("door-init")) {
      localStorage.setItem("loca-seated-rooms", JSON.stringify(seatedRooms));
      sessionStorage.setItem("door-init", "1");
    }
  }, { token: info.session_token, expiresAt: info.expires_at, name, seatedRooms });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
}

async function roster(request, room) {
  const r = await request.get(`/rooms/${room}/members`, { headers: MASTER });
  return (await r.json()).map(m => m.name);
}

test("opening a loca only watches it from the door; a seat is a separate, explicit step", async ({ page, request }) => {
  await provisionRooms(request, ["door-room"]);
  const info = await masterSession(request, "door-boss");
  await openAs(page, info, "door-boss");
  await page.evaluate(() => joinRoom("door-room"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);

  // Watching: the stream flows, the roster does not include us, nothing can be said.
  await expect(page.locator("#doorBar")).toBeVisible();
  await expect(page.locator("#seatToggle")).toHaveAttribute("aria-label", "Take a seat at this table");
  await expect(page.locator("#msg")).toBeDisabled();
  await expect(page.locator("#sendBtn")).toBeDisabled();
  await expect(page.locator("#stopBtn")).toBeDisabled();
  expect(await page.evaluate(() => state.ws.url)).toContain("watch=1");
  await page.waitForTimeout(300);
  expect(await roster(request, "door-room")).not.toContain("Master");

  // Someone speaks at the table; the watcher still sees it.
  const speaker = await masterSession(request, "door-speaker");
  const spoken = await request.post("/rooms/door-room/messages", {
    headers: { "x-session-token": speaker.session_token },
    data: { sender: "door-speaker", sender_type: "user", text: "heard at the door" },
  });
  expect(spoken.status(), await spoken.text()).toBeLessThan(300);
  await expect(page.locator("#feed")).toContainText("heard at the door");

  // Take a seat: now in the roster, the composer opens.
  await page.locator("#doorTakeSeat").click();
  await expect.poll(() => roster(request, "door-room")).toContain("Master");
  await expect(page.locator("#doorBar")).toBeHidden();
  await expect(page.locator("#seatToggle")).toHaveAttribute("aria-label", "Step out to the door");
  await expect(page.locator("#msg")).toBeEnabled();
  await page.locator("#msg").fill("now at the table");
  await page.locator("#sendBtn").click();
  await expect(page.locator("#feed")).toContainText("now at the table");

  // The choice is remembered for this loca across a reload.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("door-room"));
  await expect.poll(() => roster(request, "door-room")).toContain("Master");
  await expect(page.locator("#doorBar")).toBeHidden();

  // Step out: back to the door, gone from the roster.
  await page.locator("#seatToggle").click();
  await expect.poll(() => roster(request, "door-room")).not.toContain("Master");
  await expect(page.locator("#doorBar")).toBeVisible();
  await expect(page.locator("#msg")).toBeDisabled();
});

test("a watcher cannot send even if the composer is forced", async ({ page, request }) => {
  await provisionRooms(request, ["door-forced"]);
  const info = await masterSession(request, "door-forcer");
  await openAs(page, info, "door-forcer");
  await page.evaluate(() => joinRoom("door-forced"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => { $("msg").disabled = false; $("msg").value = "sneaky"; send(); });
  await expect(page.locator("#feed")).toContainText("watching from the door");
  const reader = await masterSession(request, "door-reader");
  const fetched = await request.get("/rooms/door-forced/messages", { headers: { "x-session-token": reader.session_token } });
  expect(fetched.ok()).toBe(true);
  const history = await fetched.json();
  const list = Array.isArray(history) ? history : (history.messages || []);
  expect(list.some(m => m.text === "sneaky")).toBe(false);
});

test("members are unaffected: no door bar, no seat icon", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => { state.adminSession = false; state.room = "x"; renderDoor(); });
  await expect(page.locator("#doorBar")).toBeHidden();
  await expect(page.locator("#seatToggle")).toBeHidden();
  expect(await page.evaluate(() => isWatchingDoor())).toBe(false);
});

test("İye is the ranks' own house: they sit there by default and may step out", async ({ page, request }) => {
  const info = await masterSession(request, "iye-boss");
  await openAs(page, info, "iye-boss");
  const home = await page.evaluate(() => state.homeRoom);
  await page.evaluate((room) => joinRoom(room), home);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  expect(await page.evaluate(() => state.ws.url)).not.toContain("watch=1");
  await expect.poll(() => roster(request, home)).toContain("Master");
  await expect(page.locator("#doorBar")).toBeHidden();
  await expect(page.locator("#msg")).toBeEnabled();
  await expect(page.locator("#seatToggle")).toHaveAttribute("aria-label", "Step out to the door");

  // Stepping out of İye is remembered like any other seat choice.
  await page.locator("#seatToggle").click();
  await expect.poll(() => roster(request, home)).not.toContain("Master");
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => joinRoom(room), home);
  await expect(page.locator("#doorBar")).toBeVisible();
  await page.locator("#doorTakeSeat").click();
  await expect.poll(() => roster(request, home)).toContain("Master");
});

test("the header keeps the seat icon beside Properties, never a text button", async ({ page, request }) => {
  await provisionRooms(request, ["door-header"]);
  const info = await masterSession(request, "door-header-boss");
  await openAs(page, info, "door-header-boss");
  await page.evaluate(() => joinRoom("door-header"));
  await expect(page.locator("#seatToggle")).toBeVisible();
  await expect(page.locator("#adminToggle")).toBeVisible();
  expect((await page.locator("#seatToggle").innerText()).trim()).toBe("");
  expect((await page.locator("#adminToggle").innerText()).trim()).toBe("");
  // Side by side: the seat icon directly precedes Properties.
  expect(await page.evaluate(() => $("seatToggle").nextElementSibling === $("adminToggle"))).toBe(true);
  // The room name stays on one line.
  const box = await page.locator("#curRoom").boundingBox();
  expect(box.height).toBeLessThan(24);
});

test("the door bar belongs to a loca, never to the Building view", async ({ page, request }) => {
  await provisionRooms(request, ["door-building"]);
  const info = await masterSession(request, "door-building-boss");
  await openAs(page, info, "door-building-boss");
  await page.evaluate(() => joinRoom("door-building"));
  await expect(page.locator("#doorBar")).toBeVisible();
  await page.evaluate(() => switchTab("people"));
  await expect(page.locator("#doorBar")).toBeHidden();
  await page.evaluate(() => switchTab("chat"));
  await expect(page.locator("#doorBar")).toBeVisible();
});
