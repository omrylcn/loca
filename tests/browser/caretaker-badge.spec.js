const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The Building's caretaker (the agent the server declares in /health
// `loca_agents`) wears a "caretaker" badge on its lines in every loca, not
// only in Care. Operator is a loca's own title; caretaker is the Building's.
// The page learns who that is from its /health poll, which a door sign-in
// started and a page restored from a stored session did not: there, no line
// ever wore the badge.

const MASTER = { "x-admin-token": "MASTER" };

test("the caretaker's lines carry a caretaker badge in an ordinary loca; nobody else's do", async ({ page, request }) => {
  const tag = Date.now().toString(36);
  const room = `ct-room-${tag}`, keeper = `ct-keeper-${tag}`;
  await provisionRooms(request, [room]);
  const admitted = await request.post("/members", { headers: MASTER, data: { name: keeper, kind: "agent" } });
  expect(admitted.ok(), await admitted.text()).toBeTruthy();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name: keeper } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const keeperSession = await (await request.post("/sessions", {
    headers: { "x-room-token": (await invited.json()).token }, data: { name: keeper, kind: "agent" },
  })).json();

  // The page is told this agent is the Building's caretaker, as a Building
  // whose LOCA_AGENT names it would tell it in /health.
  await page.route("**/health", async route => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({ response, json: { ...body, loca_agents: [keeper] } });
  });
  await page.addInitScript((room) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, room);
  // The Master signs in at the door (which starts the /health poll).
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => state.locaAgents)).toEqual([keeper]);

  const said = await request.post(`/rooms/${room}/messages`, {
    headers: { "x-session-token": keeperSession.session_token },
    data: { sender: keeper, sender_type: "agent", text: "ct the caretaker speaks" },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  await page.fill("#msg", "ct the master speaks");
  await page.keyboard.press("Enter");

  const keeperLine = page.locator("#feed .row", { hasText: "ct the caretaker speaks" });
  await expect(keeperLine.locator(".roletags .caretakertag")).toHaveText("caretaker");
  const masterLine = page.locator("#feed .row", { hasText: "ct the master speaks" });
  await expect(masterLine).toBeVisible();
  await expect(masterLine.locator(".caretakertag")).toHaveCount(0);
});

test("a page restored from a stored session shows the caretaker badge too", async ({ page, request }) => {
  const tag = Date.now().toString(36);
  const room = `ct-restored-${tag}`, keeper = `ct-keeper-r-${tag}`;
  await provisionRooms(request, [room]);
  expect((await request.post("/members", { headers: MASTER, data: { name: keeper, kind: "agent" } })).ok()).toBeTruthy();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name: keeper } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const keeperSession = await (await request.post("/sessions", {
    headers: { "x-room-token": (await invited.json()).token }, data: { name: keeper, kind: "agent" },
  })).json();
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const master = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();

  await page.route("**/health", async route => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({ response, json: { ...body, loca_agents: [keeper] } });
  });
  await page.addInitScript(({ room, token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "MASTER" }));
  }, { room, token: master.session_token, expiresAt: master.expires_at });
  // No door: the stored session is resolved and the page opens signed in.
  await page.goto("/");
  await expect(page.locator("body")).not.toHaveClass(/locked|auth-pending/);
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => state.locaAgents)).toEqual([keeper]);

  const said = await request.post(`/rooms/${room}/messages`, {
    headers: { "x-session-token": keeperSession.session_token },
    data: { sender: keeper, sender_type: "agent", text: "ct restored: the caretaker speaks" },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  const keeperLine = page.locator("#feed .row", { hasText: "ct restored: the caretaker speaks" });
  await expect(keeperLine.locator(".roletags .caretakertag")).toHaveText("caretaker");
});
