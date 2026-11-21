const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// This Loca beside the chat (from 1400px): its left edge is a grip that
// widens or narrows the panel, the width is remembered on this browser, a
// seat's name keeps room to be read beside its badges and actions, and the
// header's Info button is not shown there (the panel is already in view).

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "locainfo-width";

async function masterAt(browser, request, viewport) {
  await provisionRooms(request, [ROOM]);
  for (const name of ["muratkistan-like-name", "omnelo-dev-like"]) {
    const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
    expect([200, 201]).toContain(admitted.status());
    const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } });
    const text = await invited.text();
    expect(invited.ok() || text.includes("already holds"), text).toBeTruthy();
  }
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport });
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room: ROOM });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => window.joinRoom(room), ROOM);
  await expect(page.locator("#curRoom")).toHaveText(ROOM);
  await expect(page.locator("body")).toHaveClass(/locainfo-on/);
  return page;
}

test("This Loca beside the chat can be widened from its left edge, and stays so", async ({ browser, request }) => {
  const page = await masterAt(browser, request, { width: 1600, height: 900 });
  const panel = page.locator("#locaInfo");
  const grip = page.locator("#locaInfoResize");
  await expect(grip).toHaveAttribute("role", "separator");
  // No Info button where the panel is already in view.
  await expect(page.locator("#locaInfoBtn")).toBeHidden();

  const before = (await panel.boundingBox()).width;
  await grip.focus();
  await page.keyboard.press("ArrowLeft");
  await expect.poll(async () => (await panel.boundingBox()).width).toBeGreaterThan(before + 20);
  const widened = Math.round((await panel.boundingBox()).width);

  // By pointer too: dragging the grip left widens it further.
  const box = await grip.boundingBox();
  await page.mouse.move(box.x + 2, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x - 60, box.y + 100, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox()).width).toBeGreaterThan(widened + 40);
  const dragged = Math.round((await panel.boundingBox()).width);

  // Remembered on this browser.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => window.joinRoom(room), ROOM);
  await expect(page.locator("body")).toHaveClass(/locainfo-on/);
  await expect.poll(async () => Math.round((await page.locator("#locaInfo").boundingBox()).width))
    .toBeGreaterThanOrEqual(dragged - 2);
  await page.close();
});

test("beside the chat a seat's name keeps room to be read", async ({ browser, request }) => {
  const page = await masterAt(browser, request, { width: 1440, height: 900 });
  await expect(page.locator("#locaInfo #onlineList .omem").first()).toBeVisible();
  // Nine characters of the name at least, and no badge under an action.
  const rows = await page.evaluate(() => [...document.querySelectorAll("#locaInfo #onlineList .omem")].map(row => {
    const name = row.querySelector(".oname");
    const ch = parseFloat(getComputedStyle(name).fontSize) * 0.55;
    const badges = row.querySelector(".obadges")?.getBoundingClientRect();
    const acts = row.querySelector(".seatacts")?.getBoundingClientRect();
    return {
      text: row.textContent.trim().slice(0, 20),
      nameRoom: name.getBoundingClientRect().width >= 9 * ch - 1,
      overlap: !!(badges && acts && badges.width > 0 && acts.width > 0 && badges.right > acts.left + 0.5),
    };
  }));
  for (const row of rows) expect(row, row.text).toEqual({ text: row.text, nameRoom: true, overlap: false });
  await page.close();
});
