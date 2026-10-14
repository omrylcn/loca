const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

test("real Wiki send and editor request persist in room history without claiming model acceptance", async ({ page, request }) => {
  const room = "wiki-send-e2e", admin = { "x-admin-token": "MASTER" };
  await provisionRooms(request, [room]);
  const agents = {};
  for (const name of ["wiki-writer", "wiki-reader"]) {
    expect((await request.post("/members", { headers: admin, data: { name, kind: "agent" } })).ok()).toBeTruthy();
    const invite = await (await request.post(`/rooms/${room}/invites`, { headers: admin, data: { name } })).json();
    const session = await (await request.post("/sessions", {
      headers: { "x-room-token": invite.token }, data: { name, kind: "agent", loca: room },
    })).json();
    agents[name] = { "x-session-token": session.session_token };
  }
  expect((await request.put(`/rooms/${room}/wiki/config`, {
    headers: admin, data: { editor: "wiki-writer", enabled: false, interval_messages: 50 },
  })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/wiki/pages/working`, {
    headers: admin, data: { expected_revision: 0, title: "Deployment rules", body: "Deploy only after approval." },
  })).ok()).toBeTruthy();
  const pair = await (await request.post("/pairings?ttl_hours=1", { headers: admin })).json();
  const operator = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pair.pairing_code }, data: { name: "operator", kind: "user" },
  })).json();
  await page.addInitScript(({token,expiresAt}) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "operator" }));
  }, { token: operator.session_token, expiresAt: operator.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  // Use actual authenticated agent sockets. Single deliveries are `msg`, not
  // `turn` (the latter holds a batch). The passive receipt must not address a
  // reader; the request must reach only the assigned editor exactly once.
  await page.evaluate(async ({room, agents}) => {
    window.wikiProbeSockets = {};
    window.wikiProbeFrames = {};
    await Promise.all(Object.entries(agents).map(([name, headers]) => new Promise((resolve, reject) => {
      const url = `${wsBase()}/ws?room=${room}&name=${name}&type=agent&filter=mentions&turn_max=1`;
      const socket = new WebSocket(url, ["loca.v1", `loca.session.${headers["x-session-token"]}`]);
      wikiProbeSockets[name] = socket; wikiProbeFrames[name] = [];
      socket.onmessage = event => wikiProbeFrames[name].push(JSON.parse(event.data));
      socket.onopen = resolve; socket.onerror = () => reject(new Error("Agent probe socket failed"));
    })));
  }, {room, agents});
  await page.locator("#tabWiki").click();
  await page.locator("#wikiIndex button").filter({hasText:"Deployment rules"}).click();
  await page.locator("#wikiSendPage").click();
  await page.locator("#wikiShareTarget").selectOption("wiki-reader");
  // Simulate a response lost AFTER the server commits. Retry must return the
  // same durable message; neither selection nor the receipt is duplicated.
  let lost = false, requests = [];
  await page.route(`**/rooms/${room}/messages`, async route => {
    if (route.request().method() !== "POST") return route.continue();
    requests.push(route.request().postDataJSON());
    if (!lost) {
      lost = true;
      const accepted = await route.fetch(); expect(accepted.status()).toBe(201);
      return route.abort("failed");
    }
    return route.continue();
  });
  await page.locator("#wikiShareSend").click();
  await expect(page.locator("#wikiShareStatus")).toContainText("receipt is unconfirmed");
  await page.locator("#wikiShareSend").click();
  await expect(page.locator("#wikiShareStatus")).toContainText("Model-context acceptance is not yet confirmed");
  expect(requests).toHaveLength(2); expect(requests[0]).toEqual(requests[1]);
  const selection = await (await request.get(`/rooms/${room}/wiki/context`, { headers: agents["wiki-reader"] })).json();
  expect(selection.slugs).toEqual(["working"]);
  expect(selection.pages.map(p=>p.body)).toEqual(["Deploy only after approval."]);
  await page.locator("#wikiRequestUpdate").click();
  await page.locator("#wikiUpdateText").fill("Clarify the rollback procedure.");
  await page.locator("#wikiUpdateSend").click();
  await expect(page.locator("#wikiUpdateStatus")).toContainText("Publication is not yet confirmed");
  await page.locator("#tabChat").click();
  await expect(page.locator("#feed")).toContainText("Wiki page");
  await expect(page.locator("#feed")).toContainText("Clarify the rollback procedure.");
  // Refresh history from the server, not the POST response or browser state.
  const history = await (await request.get(`/rooms/${room}/messages?since=0`, { headers: agents["wiki-reader"] })).json();
  const sent = history.filter(m=>m.text.startsWith("Wiki page `working` selected"));
  const updates = history.filter(m=>m.text.startsWith("Wiki update request"));
  expect(sent).toHaveLength(1); expect(sent[0].kind).toBe("announce"); expect(sent[0].target ?? null).toBeNull();
  expect(sent[0].text).not.toContain("@wiki-reader");
  expect(updates).toHaveLength(1); expect(updates[0].target).toBe("wiki-writer");
  await expect.poll(() => page.evaluate(() => wikiProbeFrames["wiki-writer"].filter(f=>f.t==="msg").length)).toBe(1);
  expect(await page.evaluate(() => wikiProbeFrames["wiki-writer"].find(f=>f.t==="msg").message.text)).toContain("Clarify the rollback procedure.");
  expect(await page.evaluate(() => wikiProbeFrames["wiki-reader"].filter(f=>f.t==="msg" || f.t==="turn").length)).toBe(0);
  await page.evaluate(() => Object.values(wikiProbeSockets).forEach(socket=>socket.close()));
  await page.reload();
  await page.evaluate(room=>joinRoom(room),room);
  await expect(page.locator("#feed")).toContainText("Clarify the rollback procedure.");
  const after = await (await request.get(`/rooms/${room}/wiki`, {headers:agents["wiki-reader"]})).json();
  expect(after.revision).toBe(1); // Asking does not falsely claim a publication.
});
