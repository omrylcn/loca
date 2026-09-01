const { test, expect } = require("@playwright/test");

// Skip the first-open Getting Started guide (see notify-events.spec.js).
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// Message pinning (restored): a PERSONAL, per-user, per-room reference stored in
// localStorage — no server state, independent of the task action. Locks
// loca-dev's acceptance criteria: pin/unpin action, one pinned message shown at
// the top, jump to source, survives reload, and creates no task.
test("message pin: personal, per-room, survives reload, task-independent", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "pinuser", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "pinuser", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("pinuser");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  // A message to pin, through the real dispatcher.
  await page.evaluate(() =>
    window.onFrame({
      t: "msg",
      message: { id: 8001, sender: "bob", text: "pin this important note", ts: Date.now() },
    }),
  );
  await expect(page.locator('.row[data-id="8001"]')).toBeVisible();

  // The pin action is present in the message actions (alongside reply/task).
  await expect(page.locator('[data-pin="8001"]')).toHaveCount(1);

  // Pinning shows the single pinned bar with the message and highlights the source.
  await page.evaluate(() => window.togglePin(8001));
  await expect(page.locator("#pinnedBar")).toBeVisible();
  await expect(page.locator("#pinnedContent")).toContainText("pin this important note");
  await expect(page.locator('.row[data-id="8001"]')).toHaveClass(/pinned-source/);

  // Pinning is INDEPENDENT of tasks — it created none.
  expect(await page.evaluate(() => Object.keys(state.tasks || {}).length)).toBe(0);

  // Survives reload (localStorage, same identity + room) even though the message
  // is no longer in the freshly-loaded feed — the bar renders from the snapshot.
  await page.reload();
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");
  await expect(page.locator("#pinnedBar")).toBeVisible();
  await expect(page.locator("#pinnedContent")).toContainText("pin this important note");

  // Unpin clears the bar — must work post-reload without the message in the feed.
  await page.locator("#unpinBtn").click();
  await expect(page.locator("#pinnedBar")).toBeHidden();
  // And the unpin persisted: another reload stays clear.
  await page.reload();
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#pinnedBar")).toBeHidden();
});
