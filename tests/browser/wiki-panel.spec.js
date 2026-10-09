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

const snapshot = { room: "wiki-a", editor_name: "writer", revision: 2, reviewed_through: 10,
  reviewed_at: 1700000000000, edited_at: 1690000000000, interval_messages: 30,
  pages: [{ slug: "overview", title: "Overview", body: '<img src=x onerror="window.wikiXss=true">', sources: [10] },
    { slug: "working", title: "Working area", body: "Open proposal", sources: [] }] };

test("wiki renders sources as data and separates review from editing", async ({ page }) => {
  await expect(page.locator("#tabMemory")).toHaveCount(0);
  await expect(page.locator("#tabWiki")).toHaveText("Loca Wiki");
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

test("editor selection is roster-based and settings stay out of the document", async ({ page }) => {
  await page.evaluate(() => {
    window.isLocaOperator = () => true;
    state.members = [{ name: "writer", type: "agent" }, { name: "reader", type: "agent" }, { name: "human", type: "user" }];
  });
  let editor = "writer";
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: { ...snapshot, editor_name: editor } }));
  await page.route("**/rooms/wiki-a/wiki/config", route => {
    const body = route.request().postDataJSON();
    expect(body).toEqual({ editor: "reader", enabled: false, interval_messages: 30 });
    editor = body.editor;
    return route.fulfill({ status: 204 });
  });
  await page.evaluate(() => switchTab("wiki"));
  await expect(page.locator("#wikiEditor")).toHaveValue("writer");
  await expect(page.locator("#wikiEditor option")).toHaveText(["Choose another editor", "reader", "writer"]);
  await expect(page.locator("#wikiConfig")).toBeHidden();
  await expect(page.locator("#wikiEditorControl")).toBeVisible();
  await expect(page.locator("input[placeholder='Exact agent name']")).toHaveCount(0);
  await page.locator("#wikiEditor").selectOption("reader");
  await expect(page.locator("#wikiEditor")).toHaveValue("reader");
  await page.locator("#wikiSettingsToggle").click();
  await expect(page.locator("#wikiConfig")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("#wikiConfig")).toBeHidden();
  await page.locator("#wikiIndex button").filter({ hasText: "Working area" }).click();
  await expect(page.locator("#wikiPages h1")).toHaveText("Working area");
  await expect(page.locator("#wikiPages")).toContainText("Open proposal");
});

test("unconfigured wiki has a clean empty state on mobile", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => {
    window.isLocaOperator = () => true;
    state.members = [{ name: "writer", type: "agent" }];
    document.querySelector(".side").classList.add("hidden");
    document.body.style.gridTemplateColumns = "1fr";
  });
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ status: 404, body: "wiki not configured" }));
  await page.evaluate(() => switchTab("wiki"));
  await expect(page.locator("#wikiPages")).toContainText("This loca's wiki has not been prepared yet.");
  await expect(page.locator("#wikiConfig")).toBeHidden();
  await expect(page.locator("#wikiInterval")).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
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
