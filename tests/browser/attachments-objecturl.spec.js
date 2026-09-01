const { test, expect } = require("@playwright/test");

// The Getting Started guide auto-shows on a fresh browser and its backdrop
// intercepts clicks; skip it (see notify-events.spec.js).
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// Object-URL lifecycle (loca-dev NO-GO): an inline attachment image creates a
// blob object URL; it must be revoked — on image load, and swept on feed
// repaint / room change — so blob memory never accumulates for the life of the
// (Desktop) process. This spies on URL.createObjectURL/revokeObjectURL and
// stubs ONLY the attachment GET so an image renders without a real upload.
test("inline attachment object URLs are revoked — no blob leak on load or repaint", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "attuser", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "attuser", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("attuser");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  // Install the spy + attachment-GET stub AFTER connecting, so only the render
  // path under test is observed.
  await page.evaluate(() => {
    window.__u = { created: 0, revoked: 0, live: new Set() };
    const realCreate = URL.createObjectURL.bind(URL);
    const realRevoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob) => {
      const u = realCreate(blob);
      window.__u.created++;
      window.__u.live.add(u);
      return u;
    };
    URL.revokeObjectURL = (u) => {
      if (window.__u.live.delete(u)) window.__u.revoked++;
      return realRevoke(u);
    };
    // A real 1x1 PNG so the <img> actually fires `load` (not `error`).
    const b64 =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const realFetch = window.fetch.bind(window);
    window.fetch = (url, opts) =>
      typeof url === "string" && url.includes("/attachments/")
        ? Promise.resolve(
            new Response(new Blob([bytes], { type: "image/png" }), {
              status: 200,
              headers: { "content-type": "image/png" },
            }),
          )
        : realFetch(url, opts);
  });

  // Drive an inline-image message through the real dispatcher.
  await page.evaluate(() =>
    window.onFrame({
      t: "msg",
      message: {
        id: 7001,
        sender: "bob",
        text: "a picture",
        ts: Date.now(),
        attachments: [
          { id: "a".repeat(64), sha256: "a".repeat(64), name: "p.png", mime: "image/png", size: 70 },
        ],
      },
    }),
  );

  // One object URL is created for the image, then freed once it loads (the
  // browser keeps the decoded bitmap), so no URL stays live.
  await expect.poll(() => page.evaluate(() => window.__u.created)).toBeGreaterThan(0);
  await expect.poll(() => page.evaluate(() => window.__u.live.size)).toBe(0);
  expect(await page.evaluate(() => window.__u.revoked)).toBe(
    await page.evaluate(() => window.__u.created),
  );

  // A repaint re-renders the transcript (new URL) but must again leave zero
  // live URLs — the sweep runs on clear, so nothing accumulates round to round.
  const createdBefore = await page.evaluate(() => window.__u.created);
  await page.evaluate(() => window.repaintFeed());
  await expect.poll(() => page.evaluate(() => window.__u.created)).toBeGreaterThan(createdBefore);
  await expect.poll(() => page.evaluate(() => window.__u.live.size)).toBe(0);

  // And a room change sweeps too: switch away and assert nothing is left live.
  await page.evaluate(() => window.joinRoom("general"));
  await expect.poll(() => page.evaluate(() => window.__u.live.size)).toBe(0);
});
