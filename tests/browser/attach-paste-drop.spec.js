const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// A file reaches the composer the way it does in other chat windows: pasted
// with Ctrl+V into the message box (a screenshot is an image on the
// clipboard), or dragged onto the message box as well as onto the feed. Each
// stages the file exactly as the 📎 picker does. A paste that carries only
// text is left to the browser.

let seq = 0;

test.beforeEach(async ({ page, request }) => {
  const name = `paster${Date.now().toString(36)}${seq++}`;
  await provisionRooms(request, ["e2e", "general"]);
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name, kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name, kind: "user" },
  });
  const { token } = await invited.json();
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");
});

// A real 1x1 PNG, so the server's sniff agrees with image/png.
const PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

// Dispatch `kind` ("paste" or "drop") on #msg carrying one file, and report
// whether the page took the event over.
async function sendFile(page, kind, name) {
  return page.evaluate(
    ({ kind, name, b64 }) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], name, { type: "image/png" }));
      const init = { bubbles: true, cancelable: true };
      const ev =
        kind === "paste"
          ? new ClipboardEvent("paste", { ...init, clipboardData: dt })
          : new DragEvent("drop", { ...init, dataTransfer: dt });
      return !document.getElementById("msg").dispatchEvent(ev);
    },
    { kind, name, b64: PNG },
  );
}

test("an image pasted into the message box is staged to send", async ({ page }) => {
  await page.locator("#msg").fill("look");
  expect(await sendFile(page, "paste", "image.png")).toBe(true);
  const chip = page.locator("#attachStrip .pendingchip");
  await expect(chip).toHaveCount(1);
  await expect(chip.locator(".pname")).toHaveText("image.png");
  await expect(page.locator("#msg")).toHaveValue("look");
});

test("a file dropped on the message box is staged to send", async ({ page }) => {
  expect(await sendFile(page, "drop", "shot.png")).toBe(true);
  const chip = page.locator("#attachStrip .pendingchip");
  await expect(chip).toHaveCount(1);
  await expect(chip.locator(".pname")).toHaveText("shot.png");
});

test("a paste of text alone is left to the browser", async ({ page }) => {
  const taken = await page.evaluate(() => {
    const dt = new DataTransfer();
    dt.setData("text/plain", "hello");
    const ev = new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: dt });
    return !document.getElementById("msg").dispatchEvent(ev);
  });
  expect(taken).toBe(false);
  await expect(page.locator("#attachStrip")).toBeHidden();
});
