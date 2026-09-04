const { test, expect } = require("@playwright/test");

// Skip the first-open Getting Started guide (see notify-events.spec.js).
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// A message bubble is for conversation, not project management: the per-message
// "→ task" action was removed (it did not fit loca's spirit). This locks that
// removal — reply/pin/react stay, but no affordance turns a message into a task,
// and rendering a message creates no task.
test("message actions offer reply/pin/react but NOT a task action", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "actuser", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "actuser", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("actuser");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  // A message from someone else, through the real dispatcher (so react is shown).
  await page.evaluate(() =>
    window.onFrame({
      t: "msg",
      message: { id: 7401, sender: "bob", text: "just a chat message", ts: Date.now() },
    }),
  );
  await expect(page.locator('.row[data-id="7401"]')).toBeVisible();

  // Reply, pin, and react remain; the task action is gone entirely.
  await expect(page.locator('[data-reply="7401"]')).toHaveCount(1);
  await expect(page.locator('[data-pin="7401"]')).toHaveCount(1);
  await expect(page.locator('[data-reactpick="7401"]')).toHaveCount(1);
  await expect(page.locator('[data-mktask="7401"]')).toHaveCount(0);
  await expect(page.locator("#feed [data-mktask]")).toHaveCount(0);

  // No task was created by rendering the message.
  expect(await page.evaluate(() => Object.keys(state.tasks || {}).length)).toBe(0);
});
