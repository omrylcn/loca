const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

test.beforeEach(async ({ page, request }) => {
  await provisionRooms(request, ["entry-master"]);
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

test("Master pairing asks for neither a name nor a member type", async ({ page, request }) => {
  const pairing = await request.post("/pairings", { headers: { "x-admin-token": "MASTER" }, data: {} });
  expect(pairing.ok()).toBeTruthy();
  const { pairing_code } = await pairing.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await expect(page.locator("#name")).toBeHidden();
  await expect(page.locator("#entryKind")).toBeHidden();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => doConnect("entry-master"));
  const profile = await page.evaluate(async () =>
    (await fetch("/profile?room=entry-master", { headers: adminHeaders({}) })).json());
  expect(profile.building_role).toBe("master");
  expect(profile.principal.id).toBeTruthy();
  await page.locator("#newRoomName").fill("browser-created-loca");
  await page.locator("#newRoomBtn").click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("browser-created-loca");
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  const assignment = await (await request.get("/rooms/browser-created-loca/operators", {
    headers: { "x-admin-token": "MASTER" },
  })).json();
  expect(assignment.appointed.principal_id).toBe(profile.principal.id);
});

test("member entry offers Human and Smaster, not a self-selected Master role", async ({ page }) => {
  await page.goto("/");
  expect(await page.locator("#entryKind option").evaluateAll(rows => rows.map(row => row.value)))
    .toEqual(["human", "smaster"]);
  await expect(page.locator("#name")).toHaveValue("");
  await page.locator("#entryKind").selectOption("smaster");
  await expect(page.locator("#joinRequestBtn")).toBeHidden();
  await expect(page.locator("#pairingCode")).toBeHidden();
  await page.locator("#entryKind").selectOption("human");
  await expect(page.locator("#joinRequestBtn")).toBeVisible();
});

test("room creation appoints its founder without appointing them everywhere", async ({ page, request }) => {
  const root = { "x-admin-token": "MASTER" };
  const name = "explicit-founder-room";
  const created = await request.post("/rooms", { headers: root, data: { name } });
  expect(created.status()).toBe(201);
  const room = await created.json();
  const founder = await (await request.get(`/profile?room=${name}`, { headers: root })).json();
  expect(room.operator.principal_id).toBe(founder.principal.id);
  expect(founder.loca.operator_source).toBe("appointed");
  const other = await (await request.get("/profile?room=unrelated-room", { headers: root })).json();
  expect(other.loca.roles).not.toContain("operator");
  expect(other.loca.building_management).toBe(true);
  expect((await request.post("/rooms", { headers: root, data: { name } })).status()).toBe(409);
  const assignment = await (await request.get(`/rooms/${name}/operators`, { headers: root })).json();
  expect(assignment.appointed.principal_id).toBe(founder.principal.id);
  const member = await (await request.post("/members", { headers: root, data: { name: "not-a-founder", kind: "user" } })).json();
  const denied = await request.post("/rooms", {
    headers: { "x-admin-token": member.token }, data: { name: "unauthorized-room" },
  });
  expect(denied.status()).toBe(403);
  const raced = await Promise.all([0, 1].map(() => request.post("/rooms", {
    headers: root, data: { name: "concurrent-founder-room" },
  })));
  expect(raced.map(response => response.status()).sort()).toEqual([201, 409]);
});

test("choosing Smaster cannot elevate a Human credential", async ({ page, request }) => {
  const admitted = await request.post("/members", {
    headers: { "x-admin-token": "MASTER" }, data: { name: "ordinary-human-entry", kind: "user" },
  });
  expect(admitted.ok()).toBeTruthy();
  const member = await admitted.json();
  await page.goto("/");
  await page.locator("#entryKind").selectOption("smaster");
  await page.locator("#name").fill("pretend-master");
  await page.locator("#roomToken").fill(member.token);
  await page.evaluate(() => doConnect("entry-human"));
  const profile = await page.evaluate(async () => (await fetch("/profile", { headers: adminHeaders({}) })).json());
  expect(profile.building_role).toBe("member");
  expect(profile.principal.display_name).toBe("ordinary-human-entry");
  const create = await page.evaluate(async () => (await fetch("/rooms", {
    method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ name: "not-authorized-by-selector" }),
  })).status);
  expect(create).toBe(403);
});

test("legacy HTTP and WebSocket entry cannot create a missing loca", async ({ page, request }) => {
  const root = { "x-admin-token": "MASTER" };
  const room = "must-not-be-created-by-entry";
  const member = await (await request.post("/members", {
    headers: root, data: { name: "missing-room-probe", kind: "user" },
  })).json();
  for (const [path, data] of [
    ["invites", { name: member.name || "missing-room-probe" }],
    ["messages", { text: "must not create" }],
  ]) {
    expect((await request.post(`/rooms/${room}/${path}`, { headers: root, data })).status()).toBe(404);
  }
  const pairing = await (await request.post("/pairings", { headers: root, data: {} })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing.pairing_code }, data: { name: "", kind: "user" },
  })).json();
  expect(session.session_token).toBeTruthy();
  await page.goto("/");
  for (const watch of [false, true]) {
    const result = await page.evaluate(({ room, token, watch }) => new Promise(resolve => {
      const url = new URL("/ws", location.href);
      url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
      url.searchParams.set("room", room);
      if (watch) url.searchParams.set("watch", "1");
      const socket = new WebSocket(url, ["loca.v1", `loca.session.${token}`]);
      socket.onopen = () => { socket.close(); resolve("opened"); };
      socket.onerror = () => resolve("rejected");
    }), { room, token: session.session_token, watch });
    expect(result).toBe("rejected");
  }
  expect((await request.get(`/rooms/${room}/settings`, { headers: root })).status()).toBe(404);
  expect((await request.post("/rooms", { headers: root, data: { name: room } })).status()).toBe(201);
  expect((await request.post(`/rooms/${room}/invites`, {
    headers: root, data: { name: "missing-room-probe" },
  })).ok()).toBeTruthy();
});
