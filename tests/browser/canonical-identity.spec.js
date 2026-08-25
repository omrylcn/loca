const { test, expect } = require("@playwright/test");

test("a confirmed reaction renders without waiting for its websocket echo", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(async () => {
    state.name = "operator";
    state.room = "e2e";
    addMsg({ id: 9101, sender: "alice", sender_type: "user", text: "react here", ts: Date.now() });

    const originalFetch = window.fetch;
    window.fetch = async () => new Response(JSON.stringify({
      message_id: 9101,
      emoji: "✓",
      actors: ["operator"],
      owner: "alice",
      reactor: "operator",
      active: true,
      ts: Date.now(),
    }), { status: 200, headers: { "content-type": "application/json" } });
    try {
      await setReaction(9101, "✓");
    } finally {
      window.fetch = originalFetch;
    }
  });

  const chip = page.locator('[data-reactions="9101"] .reactionchip.mine');
  await expect(chip).toContainText("✓");
  await expect(chip.locator("span")).toHaveText("1");
});

test("a davet controls identity and ambiguous retry stays exactly once", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  const admitted = await request.post("/members", {
    headers: admin,
    data: { name: "alice", kind: "user" },
  });
  expect(admitted.status()).toBe(201);

  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "alice", kind: "user" },
  });
  expect(invited.ok()).toBeTruthy();
  const { token } = await invited.json();

  await page.goto("/");
  await expect(page.locator("#search, .searchbar")).toHaveCount(0);
  await page.locator("#name").fill("mallory");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));

  await expect(page.locator("#whoami")).toContainText("connected as alice");
  await expect(page.locator("#name")).toHaveValue("alice");
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  let postAttempts = 0;
  await page.route("**/rooms/e2e/messages", async (route) => {
    if (route.request().method() !== "POST") {
      await route.continue();
      return;
    }
    postAttempts += 1;
    if (postAttempts === 1) {
      // Commit upstream, then hide the successful response from the browser.
      // Its automatic retry must reuse op_id and receive the original row.
      const committed = await route.fetch();
      expect(committed.status()).toBe(201);
      await route.fulfill({ status: 503, body: "simulated lost upstream response" });
      return;
    }
    await route.continue();
  });

  await page.locator("#msg").fill("one canonical echo");
  await page.locator("#sendBtn").click();
  await expect(page.locator(".row.mine .body", { hasText: "one canonical echo" })).toHaveCount(1);
  await expect.poll(() => postAttempts).toBe(2);
  await page.waitForTimeout(750);
  await expect(page.locator(".row.mine .body", { hasText: "one canonical echo" })).toHaveCount(1);

  const stored = await request.get("/rooms/e2e/messages", {
    headers: { "x-room-token": token },
  });
  expect(stored.ok()).toBeTruthy();
  const matching = (await stored.json()).filter(
    (message) => message.text === "one canonical echo",
  );
  expect(matching).toHaveLength(1);

  await page.reload();
  await expect(page.locator("#whoami")).toContainText("connected as alice");
  await expect(page.locator("#name")).toHaveValue("alice");
  await expect(page.locator("#curRoom")).toHaveText("e2e");
});

test("admin properties use progressive disclosure and mobile navigation", async ({
  page,
  request,
}) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("e2e"));
  const browserProfile = await page.evaluate(async () => {
    const response = await fetch("/profile?room=e2e", { headers: adminHeaders({}) });
    return { status: response.status, body: await response.text() };
  });
  expect(browserProfile.status, browserProfile.body).toBe(200);
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await expect(page.locator("#whoami")).toContainText("OPERATOR");
  await expect(page.locator("#whoami")).toContainText("Bounded session");
  await expect(page.locator(".brandlink")).toHaveAttribute("href", "/PRINCIPLES.md");
  const principles = await request.get("/PRINCIPLES.md");
  expect(principles.status()).toBe(200);
  expect(principles.headers()["content-type"]).toContain("text/markdown");
  expect(await principles.text()).toContain("Loca");
  await expect(page.locator("#sideBuildingTab")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#sideLocaTab")).toBeVisible();
  await expect(page.locator("#sideLocaView")).toBeHidden();
  await expect(page.locator("#roomList button.room").first()).toBeVisible();
  await page.locator("#sideLocaTab").click();
  await expect(page.locator("#sideLocaTab")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#sideLocaView .online")).toBeVisible();
  await expect(page.locator("#locaSummary")).toBeVisible();
  await expect(page.locator("#locaSummary")).toContainText("Master");
  await expect(page.locator("#locaSummary")).not.toContainText("Purpose");
  await page.locator("#sideBuildingTab").click();

  await page.locator("#whoami details.profilemenu > summary").click();
  await page.locator("#whoami details.profileaccess summary").click();
  await page.locator("#credentialLabel").fill("Browser test key");
  await page.locator("#credentialCreate").click();
  await expect(page.locator("#credentialSecret")).toContainText("Copy now — shown once");
  const createdRow = page.locator(".credentialrow", { hasText: "Browser test key" });
  await expect(createdRow).toContainText("active");
  page.once("dialog", dialog => dialog.accept());
  await createdRow.locator("button", { hasText: "revoke" }).click();
  await expect(page.locator(".credentialrow", { hasText: "Browser test key" })).toContainText("revoked");

  await page.locator("#adminToggle").click();
  await expect(page.locator("#adminbar details")).toHaveCount(2);
  await expect(page.locator("#adminbar summary").nth(0)).toContainText("Room & conversation");
  await expect(page.locator("#adminbar summary").nth(1)).toContainText("Advanced agent delivery");
  await expect(page.locator("#adminbar")).not.toContainText("Follow-up & reminders");
  await expect(page.locator("#adminbar details").nth(0)).toHaveAttribute("open", "");
  await expect(page.locator("#adminbar details").nth(1)).not.toHaveAttribute("open", "");

  const inspector = await page.locator("#adminbar").boundingBox();
  const viewport = page.viewportSize();
  expect(inspector).not.toBeNull();
  expect(inspector.width).toBeLessThan(viewport.width);
  expect(inspector.x + inspector.width).toBeGreaterThan(viewport.width - 24);

  const groupBoxes = await page.locator("#adminbar details").evaluateAll((groups) =>
    groups.map((group) => ({ top: group.getBoundingClientRect().top })),
  );
  expect(groupBoxes[1].top).toBeGreaterThan(groupBoxes[0].top);

  await page.locator("#adminbar summary").nth(1).click();
  await expect(page.locator("#adminbar details").nth(0)).not.toHaveAttribute("open", "");
  await expect(page.locator("#adminbar details").nth(1)).toHaveAttribute("open", "");

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator("#sideToggle")).toBeVisible();
  await page.locator("#sideToggle").click();
  await expect(page.locator("body")).toHaveClass(/sidebar-open/);
  await page.locator("#sideBuildingTab").click();
  await expect(page.locator("#roomList button.room").first()).toBeVisible();
  await page.locator("#sideBackdrop").click({ position: { x: 380, y: 420 } });
  await expect(page.locator("body")).not.toHaveClass(/sidebar-open/);
});

test("joining a loca keeps Your Locas visible and opens chat at the latest message", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    window.__originalOpenWs = openWs;
    openWs = () => {};
    joinRoom("latest-message-room");
    onFrame({
      t: "history",
      messages: Array.from({ length: 80 }, (_, index) => ({
        id: index + 1,
        sender: "member",
        sender_type: "user",
        text: `history message ${index + 1}`,
        ts: 1_700_000_000_000 + index,
      })),
    });
  });
  await expect(page.locator("#sideBuildingTab")).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#sideLocaTab")).not.toHaveClass(/hidden/);
  await expect(page.locator("#feed .row").last()).toContainText("history message 80");
  await page.evaluate(() => {
    state.attentions = {
      stale: {
        id: "attention:latest-message-room:silence:stale",
        room: "latest-message-room",
        reason: "room_silence",
        subject: "August 13 stale reminder",
        owner: "operator",
        created_at: 1_600_000_000_000,
        delivered_at: 1_600_000_000_100,
        attempt: 1,
        status: "open",
      },
    };
    rebuildReminderChatProjection();
  });
  await expect(page.locator("#feed .row.locareminder")).toHaveCount(1);
  await expect(page.locator("#feed .row").first()).toContainText("August 13 stale reminder");
  await expect(page.locator("#feed .row").last()).toContainText("history message 80");
  const distanceFromBottom = await page.locator("#feed").evaluate(feed =>
    feed.scrollHeight - feed.scrollTop - feed.clientHeight
  );
  expect(distanceFromBottom).toBeLessThan(2);
  await page.evaluate(() => { openWs = window.__originalOpenWs; });
});

test("notes render safe Markdown by default and edit the raw source", async ({
  page,
  request,
}) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  await page.locator("#tabNotes").click();
  const key = `markdown-${Date.now()}`;
  const markdown = [
    "# Release gate",
    "",
    "- first check",
    "- **ready** now",
    "",
    "> Keep the durable record.",
    "",
    "Read [the docs](https://example.com/docs) and `verify`.",
    "",
    "<script>window.__locaNoteXss = true</script>",
  ].join("\n");
  await page.locator("#nnKey").fill(key);
  await page.locator("#nnTitle").fill("Markdown release note");
  await page.locator("#nnBody").fill(markdown);
  await page.locator("#nnCreate").click();

  const note = page.locator("#noteList .note", { hasText: key });
  await expect(note).toBeVisible();
  await expect(note.locator(".nbody h1")).toHaveText("Release gate");
  await expect(note.locator(".nbody li")).toHaveCount(2);
  await expect(note.locator(".nbody strong")).toHaveText("ready");
  await expect(note.locator(".nbody blockquote")).toContainText("Keep the durable record.");
  await expect(note.locator(".nbody a")).toHaveAttribute("href", "https://example.com/docs");
  await expect(note.locator(".nbody code.inline")).toHaveText("verify");
  await expect(note.locator(".nbody")).toContainText("<script>window.__locaNoteXss = true</script>");
  await expect(note.locator(".nbody script")).toHaveCount(0);
  expect(await page.evaluate(() => window.__locaNoteXss)).toBeUndefined();

  await note.locator("button", { hasText: "Edit" }).click();
  await expect(page.locator("#ed-body")).toHaveValue(markdown);
});

test("chat renders safe Markdown without losing mention styling", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("markdown-chat"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  const markdown = [
    "## Release check",
    "",
    "- **tests** are green",
    "- notify @lead",
    "",
    "| Gate | State |",
    "| :--- | ---: |",
    "| Browser | **green** |",
    "",
    "Read [the guide](https://example.com/guide) and `deploy`.",
    "<img src=x onerror=window.__locaChatXss=true>",
  ].join("\n");
  await page.locator("#msg").fill(markdown);
  await page.locator("#msg").press("Enter");

  const body = page.locator(".row.mine .body.chatmarkdown", { hasText: "Release check" });
  await expect(body.locator("h2")).toHaveText("Release check");
  await expect(body.locator("li")).toHaveCount(2);
  await expect(body.locator("strong").first()).toHaveText("tests");
  await expect(body.locator(".mention.leadkeyword")).toHaveText("@lead");
  await expect(body.locator("table th")).toHaveCount(2);
  await expect(body.locator("table td")).toHaveCount(2);
  await expect(body.locator("table")).toContainText("Browser");
  await expect(body.locator("a")).toHaveAttribute("href", "https://example.com/guide");
  await expect(body.locator("code.inline")).toHaveText("deploy");
  await expect(body.locator("img")).toHaveCount(0);
  expect(await page.evaluate(() => window.__locaChatXss)).toBeUndefined();
});

test("goal command, focus, optional tasks, and reminders share one human surface", async ({
  page,
  request,
}) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("focus-e2e"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  const lead = await request.post("/rooms/focus-e2e/lead", {
    headers: { "x-admin-token": "MASTER" },
    data: { lead: "operator" },
  });
  expect(lead.status()).toBe(200);

  await page.locator("#msg").fill('@goal "public release is ready"');
  await page.locator("#msg").press("Enter");
  await expect(page.locator("#goalBar")).toBeVisible();
  await expect(page.locator("#goalBar")).toContainText("public release is ready");

  await page.locator("#msg").fill("@goal public release is independently verified");
  await page.locator("#msg").press("Enter");
  await expect(page.locator("#goalBar")).toContainText("public release is independently verified");

  await page.locator("#tabTasks").click();
  await expect(page.locator("#tabTasks")).toHaveText(/Focus/);
  await expect(page.locator("#tasksPanel")).toContainText("Room goal");
  await expect(page.locator("#goalPanelCard")).toContainText("public release is independently verified");
  await expect(page.locator("#tasksPanel")).not.toContainText("Important now");
  await expect(page.locator("#focusPanelSummary, #focusPanelToggle, #search")).toHaveCount(0);
  await expect(page.locator("#tasksPanel")).toContainText("Tasks");
  await expect(page.locator("#tasksPanel")).toContainText("Reminders");
  await expect(page.locator("#taskList")).toContainText("Most work can stay in Chat");
  await expect(page.locator("#reminderSettings")).toContainText("Goal stops moving");
  await expect(page.locator("#reminderRecipient")).toContainText("@operator");
  await expect(page.locator("#reminderActiveSummary")).toContainText("Declared wait after 2 min");
  await expect(page.locator("#reminderStatus")).toHaveClass(/on/);
  await expect(page.locator("#reminderState")).toHaveText("ON");
  await expect(page.locator("#reminderLeadChoice")).toHaveAttribute("aria-checked", "true");
  await expect(page.locator("#careGoalOn")).not.toBeChecked();
  await page.locator("#careGoalOn").check();
  await page.locator("#careGoal").fill("10");
  await page.locator("#reminderPersonChoice").click();
  await expect(page.locator("#reminderPersonChoice")).toHaveAttribute("aria-checked", "true");
  await page.locator("#reminderPerson").selectOption("operator");
  await page.locator("#saveReminders").click();
  await expect(page.locator("#reminderSaveState")).toContainText("Saved");
  await expect(page.locator("#reminderActiveSummary")).toContainText("Goal after 10 min");
  const savedSettings = await request.get("/rooms/focus-e2e/settings", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(savedSettings.status()).toBe(200);
  const savedReminderSettings = await savedSettings.json();
  expect(savedReminderSettings.care_goal_secs).toBe(600);
  expect(savedReminderSettings.care_recipient).toEqual({ kind: "person", name: "operator" });
  await page.locator("#reminderAllChoice").click();
  await page.locator("#saveReminders").click();
  await expect(page.locator("#reminderSaveState")).toContainText("Saved");
  const allSettings = await request.get("/rooms/focus-e2e/settings", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect((await allSettings.json()).care_recipient).toEqual({ kind: "all" });
  await expect(page.locator("#reminderRecipient")).toContainText("everyone in this loca");

  // Durable Attention state must reconstruct a visible room receipt even when
  // the browser was offline for the original Care WebSocket frame.
  await page.evaluate(() => {
    state.attentions["attention:focus-e2e:silence:1"] = {
      id: "attention:focus-e2e:silence:1",
      reason: "room_silence",
      subject: "room has been quiet",
      owner: "operator",
      created_at: 1,
      delivered_at: 2,
      status: "open",
    };
    state.attentions["attention:focus-e2e:silence:2"] = {
      id: "attention:focus-e2e:silence:2",
      reason: "room_silence",
      subject: "room has been quiet",
      owner: "operator",
      created_at: 3,
      delivered_at: 4,
      status: "resolved",
    };
    renderReminderHistory();
    renderReminderHistory();
  });
  await expect(page.locator("#feed .reminderreceipt")).toHaveCount(0);
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow")).toHaveCount(1);
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow")).toContainText(
    "room has been quiet",
  );
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow")).toContainText("FINISHED");
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow")).toContainText("2 occurrences");

  const chatReceiptCount = await page.locator("#feed .sysline").count();
  const reminderBubbleCount = await page.locator("#feed .row.locareminder").count();
  await page.evaluate(() => onFrame({
    t: "care",
    signal: {
      state: "stalled",
      owner: "loca-care",
      subject: "lead heartbeat is missing",
    },
  }));
  await expect(page.locator("#feed .sysline")).toHaveCount(chatReceiptCount);

  await page.evaluate(() => {
    // Isolate reminder projection from the preceding command receipts; stale
    // reminder ordering against a newer transcript is covered separately.
    state.msgs = [];
    onFrame({
      t: "attention",
      attention: {
      id: "attention:focus-e2e:goal:receipt",
      reason: "goal_reminder",
      subject: "Goal: public release is independently verified",
      owner: "operator",
      created_at: 10,
      delivered_at: 20,
      attempt: 2,
      status: "open",
      },
    });
  });
  const firstReminder = page.locator("#feed .row.locareminder").last();
  await expect(firstReminder.locator(".sender")).toHaveText("loca");
  await expect(firstReminder.locator(".body")).toContainText(
    "@operator, Goal: public release is independently verified · waiting 15 min",
  );
  await expect(firstReminder.locator(".mention")).toHaveText("@operator");
  await expect(firstReminder.locator(".sender")).toHaveCSS("color", "rgb(255, 176, 102)");
  await expect(page.locator("#feed .row.locareminder")).toHaveCount(1);
  await expect(page.locator("#feed .sysline")).toHaveCount(chatReceiptCount);
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow").first()).toContainText(
    "@operator · delivered · waiting 15 min · threshold 10 min · next check 5 min · attempt 2",
  );

  const longGoal = `Goal: ${"bounded outcome ".repeat(20)}`;
  await page.evaluate(subject => onFrame({
    t: "attention",
    attention: {
      id: "attention:focus-e2e:goal:long-receipt",
      reason: "goal_reminder",
      subject,
      owner: "operator",
      created_at: 30,
      delivered_at: 40,
      attempt: 1,
      status: "open",
    },
  }), longGoal);
  await expect(page.locator("#feed .row.locareminder")).toHaveCount(1);
  await expect(page.locator("#feed .row.locareminder").last().locator(".body")).toContainText("… · waiting 10 min");
  await expect(page.locator("#reminderHistoryList .reminderhistoryrow").first()).toContainText(longGoal);

  // The server bounds new attempts. If a later/escalated delivery nevertheless
  // exists, Chat must still show that newest actionable receipt instead of
  // deleting the old bubble and filtering the replacement into invisibility.
  await page.evaluate(() => {
    state.settings.care_max_attempts = 2;
    const attemptOne = {
      id: "attention:focus-e2e:silence:attempt-1",
      room: "focus-e2e",
      reason: "room_silence",
      subject: "bounded replay reminder",
      owner: "operator",
      created_at: 45,
      delivered_at: 55,
      attempt: 1,
      status: "open",
    };
    const attemptTwo = {
      id: "attention:focus-e2e:silence:attempt-2",
      room: "focus-e2e",
      reason: "room_silence",
      subject: "bounded replay reminder",
      owner: "operator",
      created_at: 50,
      delivered_at: 60,
      attempt: 2,
      status: "open",
    };
    onFrame({ t: "attention", attention: attemptOne });
    onFrame({ t: "attention", attention: attemptOne });
    onFrame({ t: "attention", attention: attemptTwo });
    onFrame({ t: "attention", attention: attemptTwo });
    onFrame({
      t: "attention",
      attention: {
        ...attemptTwo,
        id: "attention:focus-e2e:silence:attempt-3",
        subject: "latest escalated reminder",
        created_at: 65,
        delivered_at: 70,
        attempt: 3,
      },
    });
  });
  await expect(page.locator("#feed .row.locareminder")).toHaveCount(1);
  await expect(page.locator("#feed .row.locareminder").last().locator(".body")).toContainText(
    "@operator, latest escalated reminder",
  );
  await expect(page.locator("#reminderHistoryList")).toContainText("attempt 3");

  const latestReminderBubble = page.locator("#feed .row.locareminder", { hasText: "latest escalated reminder" });
  await expect(latestReminderBubble).toHaveCount(1);

  // A stale frame from another loca is neither rendered nor admitted to this
  // room's Attention projection.
  await page.evaluate(() => onFrame({
    t: "attention",
    attention: {
      id: "attention:other-room:silence:1",
      room: "other-room",
      reason: "room_silence",
      subject: "must stay in the other loca",
      owner: "operator",
      created_at: 61,
      delivered_at: 62,
      attempt: 1,
      status: "open",
    },
  }));
  await expect(page.locator("#feed .row.locareminder", { hasText: "must stay in the other loca" })).toHaveCount(0);
  expect(await page.evaluate(() => state.attentions["attention:other-room:silence:1"])).toBeUndefined();
  // Reconnect clears the feed, then reconstructs only the latest actionable
  // reminder. All attempts remain available in the durable Reminder history.
  await page.evaluate(() => {
    $("feed").innerHTML = "";
    resetReminderChatProjection();
    rebuildReminderChatProjection();
  });
  await expect(latestReminderBubble).toHaveCount(1);

  // A → B → A and a repeated A refresh both rebuild exactly one. Replaying the
  // same live frame afterwards remains a no-op.
  await page.evaluate(() => {
    const roomAAttentions = { ...state.attentions };
    state.room = "room-b";
    state.attentions = {};
    $("feed").innerHTML = "";
    resetReminderChatProjection();
    rebuildReminderChatProjection();
    state.room = "focus-e2e";
    state.attentions = roomAAttentions;
    rebuildReminderChatProjection();
    rebuildReminderChatProjection();
    const attemptTwo = state.attentions["attention:focus-e2e:silence:attempt-2"];
    onFrame({ t: "attention", attention: attemptTwo });
    onFrame({ t: "attention", attention: attemptTwo });
  });
  await expect(latestReminderBubble).toHaveCount(1);

  const goalReceiptCount = await page.locator("#feed .sysline").count();
  await page.evaluate(() => {
    onFrame({
      t: "goal",
      goal: { id: 77, outcome: "release is independently verified", status: "active", progress_at: 100 },
    });
    onFrame({
      t: "goal",
      goal: { id: 77, outcome: "release is independently verified", status: "active", progress_at: 101 },
    });
    onFrame({
      t: "goal",
      goal: { id: 77, outcome: "release is independently verified", status: "achieved", progress_at: 102 },
    });
  });
  await expect(page.locator("#feed .sysline")).toHaveCount(goalReceiptCount + 2);
  await expect(page.locator("#feed .sysline").nth(goalReceiptCount)).toHaveText(
    "loca · Goal continues: release is independently verified",
  );
  await expect(page.locator("#feed .sysline").nth(goalReceiptCount + 1)).toHaveText(
    "loca · Goal finished: release is independently verified",
  );

  // Temporary manual focus was redundant with Goal, Tasks, and Reminders. It
  // must not leave a hidden composer or dead controls behind.
  await expect(page.locator("[data-pin]")).toHaveCount(0);
  await expect(page.locator("#pinnedBar")).toHaveCount(0);

  await expect(page.locator("#focusToggle")).toHaveCount(0);
  await expect(page.locator("#focusPanelToggle, #attentionBar, #attentionCreate")).toHaveCount(0);

  await page.locator("#tabChat").click();
  await page.locator("#msg").fill("@goal none");
  await page.locator("#msg").press("Enter");
  await expect(page.locator("#goalBar")).toBeHidden();
});

test("reminders with no room lead are visibly unavailable", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("reminder-no-lead"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  await page.locator("#msg").fill('@goal "release is verified"');
  await page.locator("#msg").press("Enter");
  await expect(page.locator("#feed")).toContainText(
    "Goal cannot be activated · Select a Lead first.",
  );
  await expect(page.locator("#goalBar")).toBeHidden();

  await page.locator("#tabTasks").click();

  await expect(page.locator("#reminderMode")).toHaveText("Reminders unavailable");
  await expect(page.locator("#reminderState")).toHaveText("NEEDS LEAD");
  await expect(page.locator("#reminderStatus")).not.toHaveClass(/on/);
  await expect(page.locator("#reminderActiveSummary")).toContainText("Select a room lead or another recipient");

  // A room can lose its lead after a reminder rule was configured. Saving
  // that form must not dead-end behind a generic 400: fall back to the room
  // audience explicitly and persist a valid policy.
  await page.locator("#saveReminders").click();
  await expect(page.locator("#reminderSaveState")).toContainText("Saved for everyone");
  await expect(page.locator("#reminderAllChoice")).toHaveAttribute("aria-checked", "true");
  const recoveredSettings = await request.get("/rooms/reminder-no-lead/settings", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(recoveredSettings.status()).toBe(200);
  expect((await recoveredSettings.json()).care_recipient).toEqual({ kind: "all" });

  const namedLead = await request.post("/rooms/reminder-no-lead/lead", {
    headers: { "x-admin-token": "MASTER" },
    data: { lead: "operator" },
  });
  expect(namedLead.status()).toBe(200);
  await expect(page.locator("#reminderMode")).toHaveText("Reminders active");
  await page.locator("#reminderLeadChoice").click();
  await page.locator("#saveReminders").click();
  await expect(page.locator("#reminderSaveState")).toContainText("Saved");

  const clearedLead = await request.post("/rooms/reminder-no-lead/lead", {
    headers: { "x-admin-token": "MASTER" },
    data: { lead: null },
  });
  expect(clearedLead.status()).toBe(200);
  await expect(page.locator("#reminderMode")).toHaveText("Reminders unavailable");
  await expect(page.locator("#reminderState")).toHaveText("NEEDS LEAD");
});

test("message actions stay attached to each bubble on desktop and mobile", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();

  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("message-actions"));
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await page.evaluate(() => {
    state.name = "operator";
    addMsg({ id: 81001, sender: "alice", sender_type: "agent", text: "short", ts: Date.now() });
    addMsg({
      id: 81002,
      sender: "alice",
      sender_type: "agent",
      text: "A deliberately long message that fills much of the available chat width without letting its actions escape to the page edge.",
      ts: Date.now(),
    });
    addMsg({ id: 81003, sender: "operator", sender_type: "user", text: "mine", ts: Date.now() });
  });

  const assertAttached = async (id) => {
    const row = page.locator(`.row[data-id="${id}"]`);
    const bubble = row.locator(".bubble");
    const actions = row.locator(".lineacts");
    await expect(actions).toHaveAttribute("role", "group");
    await expect(actions.getByRole("button", { name: /Reply to/ })).toHaveAttribute("type", "button");
    const boxes = await Promise.all([bubble.boundingBox(), actions.boundingBox(), row.boundingBox()]);
    const [bubbleBox, actionBox, rowBox] = boxes;
    expect(bubbleBox).not.toBeNull();
    expect(actionBox).not.toBeNull();
    expect(rowBox).not.toBeNull();
    expect(Math.abs(actionBox.x - (bubbleBox.x + bubbleBox.width))).toBeLessThanOrEqual(6);
    expect(Math.abs(actionBox.y - bubbleBox.y)).toBeLessThanOrEqual(6);
    expect(actionBox.x + actionBox.width).toBeLessThanOrEqual(rowBox.x + rowBox.width + 1);
  };

  await assertAttached(81001);
  await assertAttached(81002);
  await assertAttached(81003);

  const firstReply = page.locator('.row[data-id="81001"] [data-reply]');
  await firstReply.focus();
  await expect(page.locator('.row[data-id="81001"] .lineacts')).toHaveCSS("opacity", "1");
  await firstReply.press("Enter");
  await expect(page.locator("#replybar")).toBeVisible();
  await expect(page.locator("#replyText")).toContainText("replying to alice: short");

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('.row[data-id="81001"] .lineacts')).toHaveCSS("opacity", "1");
  await assertAttached(81001);
  await assertAttached(81002);
  await assertAttached(81003);
});

test("a delayed reminder fetch cannot leak across a room switch", async ({ page }) => {
  let releaseAttentionA;
  let sawAttentionA;
  const attentionAGate = new Promise(resolve => { releaseAttentionA = resolve; });
  const attentionAStarted = new Promise(resolve => { sawAttentionA = resolve; });
  const empty = route => route.fulfill({ status: 200, contentType: "application/json", body: "[]" });
  for (const endpoint of ["tasks", "goals", "waits"]) {
    await page.route(`**/rooms/race-a/${endpoint}`, empty);
    await page.route(`**/rooms/race-b/${endpoint}`, empty);
  }
  await page.route("**/rooms/race-a/attentions", async route => {
    sawAttentionA();
    await attentionAGate;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([{
        id: "attention:race-a:1",
        room: "race-a",
        reason: "room_silence",
        subject: "A-only delayed reminder",
        owner: "operator",
        created_at: 10,
        delivered_at: 20,
        attempt: 1,
        status: "open",
      }]),
    });
  });
  await page.route("**/rooms/race-b/attentions", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify([{
      id: "attention:race-b:1",
      room: "race-b",
      reason: "room_silence",
      subject: "B current reminder",
      owner: "operator",
      created_at: 30,
      delivered_at: 40,
      attempt: 1,
      status: "open",
    }]),
  }));

  await page.goto("/");
  await page.evaluate(() => {
    state.name = "operator";
    state.room = "race-a";
    // Keep this test on the REST projection boundary; joinRoom itself remains
    // real while the unrelated WebSocket transport is neutralized.
    window.__raceOpenWs = openWs;
    openWs = () => {};
    window.__raceAFetch = fetchTasks();
  });
  await attentionAStarted;
  await page.evaluate(() => joinRoom("race-b"));
  await expect(page.locator("#feed .row.locareminder", { hasText: "B current reminder" })).toHaveCount(1);
  releaseAttentionA();
  await page.evaluate(() => window.__raceAFetch);

  await expect(page.locator("#curRoom")).toHaveText("race-b");
  await expect(page.locator("#feed .row.locareminder", { hasText: "A-only delayed reminder" })).toHaveCount(0);
  await expect(page.locator("#feed .row.locareminder", { hasText: "B current reminder" })).toHaveCount(1);
  expect(await page.evaluate(() => Object.values(state.attentions).every(
    attention => !attention.room || attention.room === "race-b",
  ))).toBeTruthy();
  await page.evaluate(() => { openWs = window.__raceOpenWs; });
});

test("a delayed profile fetch cannot overwrite the current loca roles", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(paired.status()).toBe(201);
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#name").fill("operator");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("profile-race-start"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  let releaseA;
  let sawA;
  const aGate = new Promise(resolve => { releaseA = resolve; });
  const aStarted = new Promise(resolve => { sawA = resolve; });
  const profile = (name, room, roles) => JSON.stringify({
    principal: { id: `principal-${name}`, display_name: name, kind: "user" },
    building_role: name === "operator" ? "master" : "member",
    loca: { room, roles, operator_source: roles.includes("operator") ? "inherited_master" : null },
    session: { bounded: true, expires_at: Date.now() + 60_000, credential_id: "credential-test" },
  });
  await page.route("**/profile?room=profile-race-a", async route => {
    sawA();
    await aGate;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: profile("alice", "profile-race-a", ["lead", "participant"]),
    });
  });
  await page.route("**/profile?room=profile-race-b", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: profile("operator", "profile-race-b", ["operator", "participant"]),
  }));

  await page.evaluate(() => {
    state.room = "profile-race-a";
    window.delayedProfileA = fetchProfile();
  });
  await aStarted;
  await page.evaluate(async () => {
    state.room = "profile-race-b";
    await fetchProfile();
  });
  await expect(page.locator("#whoami")).toContainText("operator");
  await expect(page.locator("#whoami")).toContainText("OPERATOR");
  await expect(page.locator("#whoami")).not.toContainText("alice");

  releaseA();
  await page.evaluate(() => window.delayedProfileA);
  await expect(page.locator("#whoami")).toContainText("operator");
  await expect(page.locator("#whoami")).toContainText("OPERATOR");
  await expect(page.locator("#whoami")).not.toContainText("alice");
});

test("the loca sidebar never composes delayed facts from the previous room", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await paired.json();
  let delayA = false;
  let delayB = false;
  let releaseA;
  let releaseB;
  let aStartedCount = 0;
  let bStartedCount = 0;
  let resolveAStarted;
  let resolveBStarted;
  const aGate = new Promise(resolve => { releaseA = resolve; });
  const bGate = new Promise(resolve => { releaseB = resolve; });
  const aStarted = new Promise(resolve => { resolveAStarted = resolve; });
  const bStarted = new Promise(resolve => { resolveBStarted = resolve; });
  const roomPayload = (room, pathname) => {
    if (pathname.endsWith("/settings")) return {
      contentType: "application/json",
      body: JSON.stringify({ lead: `${room}-lead`, archived: room === "sidebar-a" }),
    };
    if (pathname.endsWith("/goals")) return {
      contentType: "application/json",
      body: JSON.stringify([{ id: 1, outcome: `${room}-purpose`, status: "active" }]),
    };
    if (pathname.endsWith("/operators")) return {
      contentType: "application/json",
      body: JSON.stringify({
        inherited_master: null,
        appointed: { display_name: `${room}-operator` },
        history: [],
      }),
    };
    if (pathname.endsWith("/invites")) return {
      contentType: "application/json",
      body: JSON.stringify([{ name: `${room}-away`, kind: "user" }]),
    };
    return {
      contentType: "application/json",
      body: JSON.stringify({
        principal: { id: `principal-${room}`, display_name: "operator", kind: "user" },
        building_role: "master",
        loca: { room, roles: [room === "sidebar-a" ? "lead" : "operator", "participant"] },
        session: { bounded: true, expires_at: Date.now() + 60_000 },
      }),
    };
  };
  await page.route(/\/(profile\?room=sidebar-[ab]|rooms\/sidebar-[ab]\/(settings|goals|operators|invites))$/, async route => {
    const url = new URL(route.request().url());
    const room = url.searchParams.get("room") || url.pathname.split("/")[2];
    const shouldDelay = room === "sidebar-a" ? delayA : delayB;
    if (shouldDelay) {
      if (room === "sidebar-a" && ++aStartedCount === 5) resolveAStarted();
      if (room === "sidebar-b" && ++bStartedCount === 5) resolveBStarted();
      await (room === "sidebar-a" ? aGate : bGate);
    }
    await route.fulfill(roomPayload(room, url.pathname));
  });

  await page.goto("/");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("sidebar-a"));
  await expect(page.locator("#locaSummary")).not.toContainText("Purpose");
  await expect(page.locator("#locaSummary")).toContainText("sidebar-a-operator");
  await expect(page.locator("#locaSummary")).toContainText("sidebar-a-lead");
  await expect(page.locator("#locaSummary")).toContainText("Closed");
  await expect(page.locator("#onlineList")).toContainText("sidebar-a-away");

  delayA = true;
  await page.evaluate(() => {
    if (state.ws) { state.ws.onclose = null; state.ws.close(); state.ws = null; }
    window.__sidebarOpenWs = openWs;
    openWs = () => {};
    window.__lateSidebarA = Promise.all([
      fetchSettings(), fetchTasks(), fetchProfile(), fetchLocaSidebar(), fetchSeated(),
    ]);
  });
  await aStarted;
  delayB = true;
  await page.evaluate(() => joinRoom("sidebar-b"));
  await bStarted;
  await expect(page.locator("#locaSummary")).toContainText("sidebar-b");
  await expect(page.locator("#locaSummary")).toContainText("Loading…");
  for (const stale of ["sidebar-a-purpose", "sidebar-a-operator", "sidebar-a-lead", "Closed", "LEAD"]) {
    await expect(page.locator("#locaSummary")).not.toContainText(stale);
  }
  await expect(page.locator("#onlineList")).not.toContainText("sidebar-a-away");

  releaseB();
  await expect(page.locator("#locaSummary")).not.toContainText("Purpose");
  await expect(page.locator("#locaSummary")).toContainText("sidebar-b-operator");
  await expect(page.locator("#locaSummary")).toContainText("sidebar-b-lead");
  await expect(page.locator("#locaSummary")).toContainText("Open");
  await expect(page.locator("#locaSummary")).toContainText("OPERATOR");
  await expect.poll(() => page.evaluate(() => JSON.stringify({ room: state.room, away: state.seatedAway, sequence: seatedFetchSequence }))).toContain("sidebar-b-away");
  await expect(page.locator("#onlineList")).toContainText("sidebar-b-away");

  releaseA();
  await page.evaluate(() => window.__lateSidebarA);
  for (const stale of ["sidebar-a-purpose", "sidebar-a-operator", "sidebar-a-lead", "Closed", "LEAD"]) {
    await expect(page.locator("#locaSummary")).not.toContainText(stale);
  }
  await expect(page.locator("#onlineList")).not.toContainText("sidebar-a-away");
  await page.evaluate(() => { openWs = window.__sidebarOpenWs; });
});

test("personal loca pin order and visibility persist without changing room lifecycle", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairingCode);
  for (const room of ["prefs-a", "prefs-b", "prefs-c"]) {
    await page.evaluate(selected => window.doConnect(selected), room);
    await expect(page.locator("#curRoom")).toHaveText(room);
  }
  await page.locator("#sideBuildingTab").click();
  await page.evaluate(() => refreshRooms());

  const item = room => page.locator("#roomList .roomitem").filter({
    has: page.locator(".rname", { hasText: room }),
  });
  const choose = async (room, action) => {
    await item(room).locator(".roompreftrigger").click();
    await item(room).locator(`[data-room-preference="${action}"]`).click();
  };
  const visibleRooms = () => page.locator("#roomList .rname").allTextContents();
  await choose("prefs-a", "pin");
  await choose("prefs-c", "pin");
  await choose("prefs-c", "up");
  let names = await visibleRooms();
  expect(names.indexOf("prefs-c")).toBeLessThan(names.indexOf("prefs-a"));

  await page.evaluate(() => { window.__roomPreferenceWs = state.ws; });
  await choose("prefs-c", "hide");
  await expect(item("prefs-c")).toHaveCount(0);
  await expect(page.locator("#hiddenLocas")).toBeVisible();
  await expect(page.locator("#hiddenRoomList")).toContainText("prefs-c");
  expect(await page.evaluate(() => ({
    room: state.room,
    sameSocket: state.ws === window.__roomPreferenceWs,
  }))).toEqual({ room: "prefs-c", sameSocket: true });
  const roomsAfterHide = await request.get("/rooms", {
    headers: { "x-admin-token": "MASTER" },
  });
  const hiddenRoom = (await roomsAfterHide.json()).find(room => room.room === "prefs-c");
  expect(hiddenRoom).toBeTruthy();
  expect(hiddenRoom.archived || false).toBe(false);

  await page.reload();
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await expect(page.locator("#curRoom")).toHaveText("prefs-c");
  await page.locator("#sideBuildingTab").click();
  await expect(item("prefs-c")).toHaveCount(0);
  await expect(page.locator("#hiddenRoomList")).toContainText("prefs-c");
  await page.locator("#hiddenLocas summary").click();
  await page.locator('#hiddenRoomList [data-room-preference="show"][data-room="prefs-c"]').click();
  await expect(item("prefs-c")).toHaveCount(1);
  await expect(page.locator("#hiddenRoomList")).not.toContainText("prefs-c");
  names = await visibleRooms();
  expect(names.indexOf("prefs-c")).toBeLessThan(names.indexOf("prefs-a"));
  await expect(page.locator("#curRoom")).toHaveText("prefs-c");
});
