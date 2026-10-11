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
  pages: [{ slug: "overview", title: "Overview", body: '<img src=x onerror="window.wikiXss=true">', sources: [10], revision: 2 },
    { slug: "working", title: "Working area", body: "Open proposal", sources: [], revision: 1 }] };

test("automatic review is explicitly saved, verified and capability failures stay visible", async ({ page }) => {
  await page.evaluate(() => { window.isLocaOperator = () => true; });
  let current={...snapshot,enabled:false}, reject=true;
  await page.route("**/rooms/wiki-a/wiki",route=>route.fulfill({json:current}));
  await page.route("**/rooms/wiki-a/wiki/config",route=> {
    if(reject) return route.fulfill({status:409});
    current={...current,...route.request().postDataJSON(),editor_name:"writer"};
    return route.fulfill({status:204});
  });
  await page.evaluate(()=>switchTab("wiki"));
  await page.locator("#wikiSettingsToggle").click();
  await page.locator("#wikiAutoEnabled").check();
  await page.locator("#wikiInterval").fill("50");
  await page.locator("#wikiPolicySave").click();
  await expect(page.locator("#wikiPolicyStatus")).toContainText("worker is not ready");
  reject=false;
  await page.locator("#wikiPolicySave").click();
  await expect(page.locator("#wikiPolicyStatus")).toContainText("enabled and verified");
  await page.locator("#wikiAutoEnabled").uncheck();
  await page.locator("#wikiPolicySave").click();
  await expect(page.locator("#wikiPolicyStatus")).toContainText("paused and verified");
});

test("manual page selection never claims model acceptance", async ({ page })=> {
  await page.evaluate(()=>{window.isLocaOperator=()=>true;});
  await page.route("**/rooms/wiki-a/wiki",route=>route.fulfill({json:snapshot}));
  await page.route("**/rooms/wiki-a/wiki/context/offer",route=> {
    expect(route.request().postDataJSON()).toEqual({target:"reader",slugs:["working"],reason:"operator selected this page"});
    return route.fulfill({json:{offered:true}});
  });
  await page.evaluate(()=>switchTab("wiki"));
  await page.locator("#wikiIndex button").filter({hasText:"Working area"}).click();
  await page.locator("#wikiSettingsToggle").click();
  await page.locator("#wikiShareTarget").fill("reader");
  await page.locator("#wikiShareSend").click();
  await expect(page.locator("#wikiShareStatus")).toContainText("acceptance is not yet confirmed");
});

test("operator page editing verifies the accepted page revision, not matching older text", async ({ page }) => {
  await page.evaluate(() => { window.isLocaOperator = () => true; });
  let saved = structuredClone(snapshot), mode = "stale", writes = 0;
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: saved }));
  await page.route("**/rooms/wiki-a/wiki/pages/overview", route => {
    if (route.request().method() === "PUT") {
      ++writes;
      expect(route.request().postDataJSON()).toEqual({ expected_revision: 2, title: "Overview", body: "Human correction" });
      return route.fulfill({ json: { room: "wiki-a", slug: "overview", revision: 3 } });
    }
    // Matching text at the old revision is not proof of this edit.
    const current = { ...saved.pages[0], body: "Human correction", sources: [], revision: mode === "stale" ? 2 : 3 };
    return route.fulfill({ json: { room: "wiki-a", revision: 3, page: current } });
  });
  await page.evaluate(() => switchTab("wiki"));
  await page.locator("#wikiEditPage").click();
  await page.locator("#wikiPageBody").fill("Human correction");
  await page.locator("#wikiPageSave").click();
  await expect(page.locator("#wikiEditStatus")).toContainText("current page differs");
  await expect(page.locator("#wikiPageBody")).toHaveValue("Human correction");
  expect(writes).toBe(1);
  mode = "current";
  saved = { ...saved, revision: 3, pages: [{ ...saved.pages[0], revision: 3, body: "Human correction", sources: [] }, saved.pages[1]] };
  await page.locator("#wikiPageSave").click();
  await expect(page.locator("#wikiStatus")).toContainText("Page saved and verified · revision 3");
  await expect(page.locator("#wikiPageEdit")).toBeHidden();
});

test("failed and conflicting edits preserve drafts and are never retried automatically", async ({ page }) => {
  await page.evaluate(() => { window.isLocaOperator = () => true; });
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: snapshot }));
  let writes = 0, status = 503;
  await page.route("**/rooms/wiki-a/wiki/pages/overview", route => { ++writes; return route.fulfill({ status }); });
  await page.evaluate(() => switchTab("wiki"));
  await page.locator("#wikiEditPage").click();
  await page.locator("#wikiPageBody").fill("My draft");
  await page.locator("#wikiPageSave").click();
  await expect(page.locator("#wikiEditStatus")).toContainText("Your draft is kept");
  expect(writes).toBe(1);
  status = 409;
  await page.locator("#wikiPageSave").click();
  await expect(page.locator("#wikiEditStatus")).toContainText("Wiki changed");
  await expect(page.locator("#wikiPageBody")).toHaveValue("My draft");
  expect(writes).toBe(2);
  await page.evaluate(() => { state.room = "wiki-b"; state.locaContext = "context-b"; resetWikiForRoom(); });
  await expect(page.locator("#wikiPageEdit")).toBeHidden();
  await expect(page.locator("#wikiPageBody")).toHaveValue("");
  await page.evaluate(() => { state.room = "wiki-a"; state.locaContext = "context-a2"; });
  await page.evaluate(() => fetchWiki());
  await page.locator("#wikiEditPage").click();
  await expect(page.locator("#wikiPageBody")).toHaveValue("My draft");
});

test("readers have no human edit control and unchanged edits accept the original page revision", async ({ page }) => {
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: snapshot }));
  await page.evaluate(() => switchTab("wiki"));
  await expect(page.locator("#wikiEditPage")).toBeHidden();
  await page.evaluate(() => { window.isLocaOperator = () => true; renderWikiEditor(); });
  await page.locator("#wikiIndex button").filter({ hasText: "Working area" }).click();
  await page.route("**/rooms/wiki-a/wiki/pages/working", route => route.fulfill({ json: route.request().method() === "PUT"
    ? { room: "wiki-a", slug: "working", revision: 2 }
    : { room: "wiki-a", revision: 2, page: snapshot.pages[1] } }));
  await page.locator("#wikiEditPage").click();
  await page.locator("#wikiPageSave").click();
  await expect(page.locator("#wikiStatus")).toContainText("Page saved and verified · revision 2");
});

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
  await expect(page.locator("#wikiInterval")).toBeHidden();
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

test("sidebar room navigation refreshes an already open Wiki and clears old pages immediately", async ({ page }) => {
  let release;
  const wait = new Promise(resolve => { release = resolve; });
  await page.route("**/rooms/wiki-a/wiki", route => route.fulfill({ json: snapshot }));
  await page.route("**/rooms/wiki-b/wiki", async route => {
    await wait;
    await route.fulfill({ json: { ...snapshot, room: "wiki-b", editor_name: "b-editor", revision: 9,
      pages: [{ slug: "overview", title: "B overview", body: "B private knowledge", sources: [] }] } });
  });
  await page.evaluate(() => {
    // Keep unrelated transports out of this navigation regression; joinRoom
    // and the actual sidebar click handler still run unchanged.
    for (const fn of ["refreshRooms", "fetchNotes", "fetchMemory", "fetchMode", "fetchSettings", "fetchMod",
      "fetchTasks", "fetchJournal", "fetchSeated", "fetchLobby", "fetchProfile", "fetchLocaSidebar", "openWs"])
      window[fn] = () => {};
    state.rooms = [{ room: "wiki-a" }, { room: "wiki-b" }];
    renderRooms();
    switchTab("wiki");
  });
  await expect(page.locator("#wikiStatus")).toContainText("Revision 2");
  await page.locator("#wikiIndex button").filter({ hasText: "Working area" }).click();
  await expect(page.locator("#wikiPages")).toContainText("Open proposal");
  const requested = page.waitForRequest("**/rooms/wiki-b/wiki");
  await page.locator("#roomList .room").filter({ hasText: "wiki-b" }).click();
  await requested;
  await expect(page.locator("#wikiPages")).not.toContainText("Open proposal");
  await expect(page.locator("#wikiIndex button")).toHaveCount(0);
  release();
  await expect(page.locator("#wikiPages h1")).toHaveText("B overview");
  await expect(page.locator("#wikiPages")).toContainText("B private knowledge");
  await expect(page.locator("#wikiStatus")).toContainText("Revision 9");
});
