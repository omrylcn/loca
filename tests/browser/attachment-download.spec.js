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
