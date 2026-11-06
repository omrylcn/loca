const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const MASTER = { "x-admin-token": "MASTER" };

async function openAsMaster(page, request) {
  const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
  const { pairing_code } = await paired.json();
  const signedIn = await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "tip-boss", kind: "user" },
  });
  const info = await signedIn.json();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "tip-boss" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
}

test("header icons explain themselves on hover and focus, and follow live and connection state", async ({ page, request }) => {
  await provisionRooms(request, ["tip-room"]);
  await openAsMaster(page, request);
  await page.evaluate(() => joinRoom("tip-room"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  const tip = page.locator("#tipBubble");

  // No native title tooltips left to double up with the styled bubble.
  for (const id of ["seatToggle", "adminToggle", "liveToggle", "connDot"]) {
    await expect(page.locator(`#${id}`)).not.toHaveAttribute("title", /.*/);
  }

  await page.locator("#connDot").hover();
  await expect(tip).toBeVisible();
  await expect(tip).toContainText("Connected");
  await expect(tip).toContainText("Live link to the server");

  await page.locator("#seatToggle").hover();
  await expect(tip).toContainText("Take a seat");

  await page.locator("#adminToggle").hover();
  await expect(tip).toContainText("Properties");

  await page.locator("#liveToggle").hover();
  await expect(tip).toContainText("Live mode is off");
  // The bubble stays inside the viewport and is described by the control.
  const box = await tip.boundingBox();
  const vw = page.viewportSize().width;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(vw);
  await expect(page.locator("#liveToggle")).toHaveAttribute("aria-describedby", "tipBubble");

  // Turning live on rewrites the toggle and its tooltip; the badge explains itself too.
  await page.locator("#liveToggle").click();
  await expect(page.locator("#liveToggle")).toHaveText("● Live: on");
  await expect(page.locator("#liveToggle")).toHaveAttribute("aria-pressed", "true");
  await page.mouse.move(0, 400);
  await page.locator("#liveToggle").hover();
  await expect(tip).toContainText("Live mode is on");
  await page.locator("#liveBadge").hover();
  await expect(tip).toContainText("Every message reaches everyone");

  // Escape closes it; keyboard focus opens it without a hover.
  await page.keyboard.press("Escape");
  await expect(tip).toBeHidden();
  await page.locator("#adminToggle").focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab");
  await expect(tip).toBeVisible();
  await expect(tip).toContainText("Properties");
});
