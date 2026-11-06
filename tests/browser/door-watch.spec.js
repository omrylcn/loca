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
  await expect(page.locator("#seatToggle")).toHaveText(/Take a seat/);
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
  await expect(page.locator("#seatToggle")).toHaveText(/Step out/);
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

test("members are unaffected: no door bar, no seat toggle", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => { state.adminSession = false; state.room = "x"; renderDoor(); });
  await expect(page.locator("#doorBar")).toBeHidden();
  await expect(page.locator("#seatToggle")).toBeHidden();
  expect(await page.evaluate(() => isWatchingDoor())).toBe(false);
});
