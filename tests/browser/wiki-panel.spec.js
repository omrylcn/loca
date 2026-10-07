const { test, expect } = require("@playwright/test");

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    document.body.classList.remove("locked");
    document.querySelector(".main").classList.remove("global");
    document.querySelector(".main").style.setProperty("display", "flex", "important");
    state.room = "wiki-a"; state.locaContext = "context-a";
  });
});

const snapshot = { room: "wiki-a", revision: 2, reviewed_through: 10,
  reviewed_at: 1700000000000, edited_at: 1690000000000, interval_messages: 30,
  pages: [{ slug: "overview", title: "Overview", body: '<img src=x onerror="window.wikiXss=true">', sources: [10] },
    { slug: "working", title: "Working area", body: "Open proposal", sources: [] }] };

test("wiki renders sources as data and separates review from editing", async ({ page }) => {
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: snapshot }));
  await page.evaluate(() => switchTab("wiki"));
  await expect(page.locator("#wikiStatus")).toContainText("Revision 2");
  await expect(page.locator("#wikiStatus")).toContainText("last review:");
  await expect(page.locator("#wikiStatus")).toContainText("last edit:");
  await expect(page.locator("#wikiPages img")).toHaveCount(0);
  await expect(page.locator("#wikiPages")).toContainText("<img src=x");
  await expect(page.locator("#wikiPages button")).toHaveText("#10");
  await expect(page.locator("#wikiConfig")).toBeHidden();
});

test("an old room fetch never renders private pages in the new room", async ({ page }) => {
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  await page.route("**/rooms/wiki-a/wiki", async route => { await wait; await route.fulfill({ json: snapshot }); });
  const requested = page.waitForRequest("**/rooms/wiki-a/wiki");
  await page.evaluate(() => { switchTab("wiki"); });
  await requested;
  await page.evaluate(() => { state.room = "wiki-b"; state.locaContext = "context-b"; });
  const responded = page.waitForResponse("**/rooms/wiki-a/wiki");
  release();
  await responded;
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  await expect(page.locator("#wikiPages")).not.toContainText("Open proposal");
});
