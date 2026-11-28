const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

test.beforeEach(async ({ page, request }) => {
  await provisionRooms(request, ["e2e", "general"]);
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// A header value cannot carry "ş", "ğ" or "ı": before the fix fetch refused to
// send the upload at all, and "ö" arrived as Latin-1 and the file was renamed
// "attachment.txt". The name now travels percent-encoded and comes back whole,
// and an HTML or JSON file is accepted as a text file.
test("a Turkish file name and an HTML file upload under their own names", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "adsahibi", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "adsahibi", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("adsahibi");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  await page.locator("#attachInput").setInputFiles([
    { name: "çalışma notu ğ.md", mimeType: "text/markdown", buffer: Buffer.from("# not\n") },
    { name: "rapor.html", mimeType: "text/html", buffer: Buffer.from("<p>rapor</p>\n") },
  ]);
  const names = page.locator("#attachStrip .pname");
  await expect(names).toHaveText(["çalışma notu ğ.md", "rapor.html"]);
});
