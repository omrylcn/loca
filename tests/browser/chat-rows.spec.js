const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Every theme draws each message with its writer's face on the left
// (an agent's * on a square, a person's initials on a circle) and its address
// as "→ @name" in front of the words.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "chat-rows";

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room: ROOM });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page;
}

test("each message wears its writer's face and its address, in every theme", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request);
  await page.evaluate(() => applyAppearance({ theme: "loca" }));
  await page.evaluate(room => {
    addMsg({ id: 91001, room, sender: "rows-agent", sender_type: "agent", text: "the agent speaks", ts: Date.now() });
    addMsg({ id: 91002, room, sender: "deniz", sender_type: "user", target: "rows-agent", text: "please check", ts: Date.now() });
  }, ROOM);
  const agentRow = page.locator("#feed .row", { hasText: "the agent speaks" });
  const personRow = page.locator("#feed .row", { hasText: "please check" });

  await expect(agentRow.locator(".face .face-agent")).toHaveText("*");
  await expect(agentRow.locator(".face")).toBeVisible();
  await expect(personRow.locator(".face .face-person .avinitials")).toHaveText("DE");

  // The address is "→ @rows-agent", in front of the words.
  const addr = personRow.locator(".body .mention.addr");
  await expect(addr).toHaveText("@rows-agent");
  expect(await addr.evaluate(el => getComputedStyle(el, "::before").content)).toBe('"→ "');

  // Faces sit in one column at the left edge of the feed.
  const [a, p] = [await agentRow.locator(".face").boundingBox(), await personRow.locator(".face").boundingBox()];
  expect(Math.abs(a.x - p.x)).toBeLessThan(1);
  expect(a.width).toBeCloseTo(36, 0);

  // Every theme reads this way: the older Dark too, which many saved
  // looks still name.
  await page.evaluate(() => applyAppearance({ theme: "dark" }));
  await expect(agentRow.locator(".face")).toBeVisible();
  expect(await addr.evaluate(el => getComputedStyle(el, "::before").content)).toBe('"→ "');
  await page.close();
});

// On a wide screen in the redesign themes, This Loca stands on the right of
// the chat and the sidebar keeps Your Locas; narrower, or in another theme,
// it goes back behind the sidebar's second tab.
test("This Loca stands beside the chat on a wide screen, in every theme", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request);
  await page.evaluate(() => applyAppearance({ theme: "loca" }));
  await page.setViewportSize({ width: 1500, height: 900 });

  const panel = page.locator("#locaInfo");
  await expect(panel).toBeVisible();
  await expect(panel.locator("#sideLocaView #onlineList")).toHaveCount(1);
  await expect(page.locator("#sideBuildingView")).toBeVisible();
  await expect(page.locator(".sideviewtabs")).toBeHidden();
  const [main, right] = [await page.locator("main.main").boundingBox(), await panel.boundingBox()];
  expect(right.x).toBeGreaterThanOrEqual(main.x + main.width - 1);
  expect(right.x + right.width).toBeLessThanOrEqual(1501);

  // Narrower: back in the sidebar, behind its tab.
  await page.setViewportSize({ width: 1280, height: 900 });
  await expect(panel).toBeHidden();
  await expect(page.locator("#sidebar #sideLocaView")).toHaveCount(1);
  await expect(page.locator(".sideviewtabs")).toBeVisible();
  await page.locator("#sideLocaTab").click();
  await expect(page.locator("#sideLocaView")).toBeVisible();

  // Wide again, in the older Dark theme: beside the chat as well.
  await page.setViewportSize({ width: 1500, height: 900 });
  await page.evaluate(() => applyAppearance({ theme: "dark" }));
  await expect(panel).toBeVisible();
  await expect(panel.locator("#sideLocaView")).toHaveCount(1);
  await page.close();
});

// Where one message ends and the next begins: an edge in the writer's
// colour (agent blue, person green), a hairline between messages.
test("each message has an edge in its writer's colour and a line between messages", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request);
  await page.evaluate(() => applyAppearance({ theme: "loca" }));
  await page.evaluate(room => {
    addMsg({ id: 92001, room, sender: "edge-agent", sender_type: "agent", text: "from an agent", ts: Date.now() });
    addMsg({ id: 92002, room, sender: "edge-person", sender_type: "user", text: "from a person", ts: Date.now() });
  }, ROOM);
  const edge = text => page.locator("#feed .row", { hasText: text }).evaluate(el => getComputedStyle(el).boxShadow);
  const tokens = await page.evaluate(() => {
    const css = getComputedStyle(document.documentElement);
    return { agent: css.getPropertyValue("--agent").trim(), human: css.getPropertyValue("--human").trim() };
  });
  expect(await edge("from an agent")).toContain("inset");
  expect(await edge("from a person")).toContain("inset");
  expect(await edge("from an agent")).not.toBe(await edge("from a person"));
  const line = await page.locator("#feed .row", { hasText: "from a person" })
    .evaluate(el => getComputedStyle(el, "::before").height);
  expect(line).toBe("1px");
  expect(tokens.agent).not.toBe(tokens.human);
  await page.close();
});
