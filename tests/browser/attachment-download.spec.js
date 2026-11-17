const { test, expect } = require("@playwright/test");
const fs = require("node:fs");

test("HTML attachment downloads without navigation and carries authentication", async ({ page }) => {
  await page.goto("/");
  // Exercise the shipped attachment functions with only the surrounding app mocked.
  const source = fs.readFileSync("web/assets/chat.js", "utf8");
  const section = source.slice(source.indexOf("const inlineAttachmentUrls"), source.indexOf("/* ---- @mention tab flash"));
  await page.evaluate((section) => {
    window.__auth = false;
    window.__opened = false;
    window.open = () => { window.__opened = true; };
    window.fetch = (url, opts) => {
      window.__auth = opts.headers["x-test-auth"] === "fixture";
      return Promise.resolve(new Response("<html>download fixture</html>", { headers: { "content-type": "text/html" } }));
    };
    const install = new Function("serverBase", "adminHeaders", "esc", "fmtBytes", "addSys", section + '\nreturn attachmentChip("fixture", {id:"fixture", name:"sample.html", size:42, mime:"text/html"});');
    document.body.appendChild(install(() => "", () => ({"x-test-auth":"fixture"}), s => s, s => String(s), () => {}));
  }, section);
  const downloadEvent = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download sample.html", exact: true }).click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("sample.html");
  expect(await download.failure()).toBeNull();
  expect(await page.evaluate(() => window.__auth)).toBe(true);
  expect(await page.evaluate(() => window.__opened)).toBe(false);
});

// The desktop window opens no second window, so a click on an inline image
// opened nothing there (desktop bug G). In the desktop shell the click saves
// the file; in a browser it still opens it in a new tab.
for (const desktop of [true, false]) {
  test(`clicking an inline image ${desktop ? "saves it in the desktop shell" : "opens it in a browser"}`, async ({ page }) => {
    await page.goto("/");
    const source = fs.readFileSync("web/assets/chat.js", "utf8");
    const section = source.slice(source.indexOf("const inlineAttachmentUrls"), source.indexOf("/* ---- @mention tab flash"));
    await page.evaluate(({ section, desktop }) => {
      window.__LOCA_DESKTOP__ = desktop;
      window.__opened = 0;
      window.__notes = [];
      window.open = () => { window.__opened += 1; };
      window.fetch = () => Promise.resolve(new Response(new Uint8Array([137, 80, 78, 71]), { headers: { "content-type": "image/png" } }));
      const install = new Function("serverBase", "adminHeaders", "esc", "fmtBytes", "addSys", "state", "buildingStale",
        section + '\nreturn () => openAttachment("fixture", {id:"fixture", name:"photo.png", size:4, mime:"image/png"});');
      window.__openIt = install(() => "", () => ({}), s => s, s => String(s), t => window.__notes.push(t), { buildingGen: 0 }, () => false);
    }, { section, desktop });
    if (desktop) {
      const downloadEvent = page.waitForEvent("download");
      await page.evaluate(() => window.__openIt());
      const download = await downloadEvent;
      expect(download.suggestedFilename()).toBe("photo.png");
      expect(await page.evaluate(() => window.__opened)).toBe(0);
    } else {
      await page.evaluate(() => window.__openIt());
      await expect.poll(() => page.evaluate(() => window.__opened)).toBe(1);
    }
    expect(await page.evaluate(() => window.__notes)).toEqual([]);
  });
}
