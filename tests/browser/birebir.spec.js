const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function rankSession(request, name) {
  if (name === "MASTER") {
    const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
    return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  }
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } })).json();
}

async function openAs(browser, info, name) {
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

test("a birebir is seen by its two only; the Master sees that it exists and reads only when called in", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "bb-sa"), "bb-sa");
  const sb = await openAs(browser, await rankSession(request, "bb-sb"), "bb-sb");
  const other = await openAs(browser, await rankSession(request, "bb-other"), "bb-other");
  const master = await openAs(browser, await rankSession(request, "MASTER"), "Master");

  // sa opens a birebir with sb: it lands under Messages, named by the peer.
  await sa.evaluate(() => openBirebir("bb-sb"));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  const room = await sa.evaluate(() => state.room);
  await expect(sa.locator("#curRoom")).toHaveText("bb-sb");
  await expect(sa.locator("#birebirBar")).toContainText("Only you and bb-sb see this");
  await expect(sa.locator("#birebirBar")).toContainText("Not encrypted");
  await expect(sa.locator(".tabs")).toBeHidden();
  await expect(sa.locator("#seatToggle")).toBeHidden();
  await expect(sa.locator("#roomList")).not.toContainText(room);
  await expect(sa.locator("#msg")).toHaveAttribute("placeholder", /say it to bb-sb/);
  await expect.poll(() => sa.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await sa.fill("#msg", "before the call");
  await sa.keyboard.press("Enter");
  await expect(sa.locator("#feed")).toContainText("before the call");
  // Every word is to the other side, so no "@peer" prefix clutters the line.
  await expect(sa.locator("#feed .row", { hasText: "before the call" })).not.toContainText("@bb-sb");

  // sb finds it under Messages and reads it.
  await sb.evaluate(() => refreshRooms());
  await expect(sb.locator("#dmList")).toContainText("bb-sa");
  await sb.locator("#dmList .room.dm", { hasText: "bb-sa" }).click();
  await expect(sb.locator("#feed")).toContainText("before the call");

  // Another rank sees nothing — no Messages entry, no loca, no door.
  await other.evaluate(() => refreshRooms());
  await expect(other.locator("#dmList")).not.toContainText("bb-s");
  expect(await other.evaluate(() => state.rooms.map(r => r.room))).not.toContain(room);
  const peek = await other.evaluate(async (room) => (await fetch(`/rooms/${room}/messages`, { headers: adminHeaders({}) })).status, room);
  expect(peek).toBe(404);

  // The Master's Building view: existence only, no Read.
  await master.evaluate(() => switchTab("people"));
  const overview = master.locator("#birebirOverview");
  await expect(overview).toContainText("bb-sa");
  await expect(overview).toContainText("bb-sb");
  await expect(overview).not.toContainText("before the call");
  await expect(overview.locator("[data-bbread]")).toHaveCount(0);

  // sb calls the Master in; both sides see it.
  await sb.locator("#birebirBar [data-bb=call]").click();
  await expect(sb.locator("#birebirBar")).toContainText("The Master is reading");
  await expect(sa.locator("#birebirBar")).toContainText("The Master is reading");
  await expect(sa.locator("#birebirBar [data-bb=dismiss]")).toHaveCount(0);
  await sa.fill("#msg", "after the call");
  await sa.keyboard.press("Enter");
  await expect(sb.locator("#feed")).toContainText("after the call");

  // The Master reads from the call on; earlier history stays closed.
  await master.evaluate(() => fetchBirebirOverview());
  await overview.locator("[data-bbread]").click();
  await expect(master.locator("#bbWitness")).toContainText("after the call");
  await expect(master.locator("#bbWitness")).not.toContainText("before the call");

  // Both allow history; then it opens.
  await sa.locator("#birebirBar [data-bb=history]").click();
  await sb.locator("#birebirBar [data-bb=history]").click();
  await expect(sb.locator("#birebirBar")).toContainText("everything, from the start");
  await overview.locator("[data-bbread]").click();
  await expect(master.locator("#bbWitness")).toContainText("before the call");

  // The caller sends the Master out.
  await sb.locator("#birebirBar [data-bb=dismiss]").click();
  await expect(sb.locator("#birebirBar")).toContainText("Only you and bb-sa see this");
  for (const p of [sa, sb, other, master]) await p.close();
});
