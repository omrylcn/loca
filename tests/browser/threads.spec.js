const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Reply threads (threads.js): a message others answered wears
// "N replies · last at HH:MM"; the thread opens in This Loca's place on a
// wide screen and over the chat on a narrower one; its reply box sends the
// composer's way with reply_to set to the thread's first message; and a
// thread whose first message is further back than the loaded chat reads it
// from the loca's history.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request, room, viewport) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport });
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  // The loca's history has been drawn: what is seeded next comes after it.
  await expect.poll(() => page.evaluate(() => document.querySelector("#feed").children.length)).toBeGreaterThan(0);
  return page;
}

function seed(page, room, messages) {
  return page.evaluate(({ room, messages }) => {
    for (const m of messages) addMsg({ room, sender_type: "user", ts: Date.now(), ...m });
  }, { room, messages });
}

test("replies count under their message and open as a thread beside the chat", async ({ browser, request }) => {
  const ROOM = "threads-wide";
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request, ROOM, { width: 1500, height: 900 });
  await seed(page, ROOM, [
    { id: 93001, sender: "reviewer", sender_type: "agent", text: "The refresh token is written to the debug log." },
    { id: 93002, sender: "deniz", text: "something else entirely" },
    { id: 93003, sender: "omer", text: "Agreed, let's make it a release rule.", reply_to: 93001 },
    { id: 93004, sender: "reviewer", sender_type: "agent", text: "Added it to the rules draft.", reply_to: 93003 },
  ]);

  // A reply to a reply belongs to the same thread; the message in between
  // wears no count of its own.
  const link = page.locator('#feed .row[data-id="93001"] .threadlink');
  await expect(link).toHaveText(/^2 replies · last at .+/);
  await expect(page.locator('#feed .row[data-id="93003"] .threadlink')).toHaveCount(0);
  await expect(page.locator('#feed .row[data-id="93002"] .threadlink')).toHaveCount(0);
  // The quote still goes to what it answers.
  await expect(page.locator('#feed .row[data-id="93003"] .quote[data-goto="93001"]')).toHaveCount(1);

  await link.click();
  const panel = page.locator("#threadPanel");
  await expect(panel).toBeVisible();
  // In This Loca's place, not over the chat.
  await expect(page.locator("#locaInfo #threadPanel")).toHaveCount(1);
  await expect(panel).not.toHaveClass(/overlay/);
  await expect(page.locator("#sideLocaView")).toBeHidden();
  await expect(page.locator("#threadBackdrop")).toBeHidden();
  await expect(panel.locator("#threadRoot")).toContainText("The refresh token is written to the debug log.");
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveText([
    /Agreed, let's make it a release rule\./,
    /Added it to the rules draft\./,
  ]);
  await expect(panel.locator("#threadEarlier")).toBeHidden();

  // Live: a new reply counts under the message and joins the open thread.
  await page.evaluate(room => onFrame({ t: "msg", message: {
    id: 93005, room, sender: "lead", sender_type: "agent", text: "Shipping it in the next tag.", reply_to: 93001, ts: Date.now(),
  } }), ROOM);
  await expect(link).toHaveText(/^3 replies · last at /);
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveCount(3);
  await expect(panel.locator("#threadReplies .threadmsg").last()).toContainText("Shipping it in the next tag.");

  // The reply box sends the composer's way, answering the thread's first
  // message, and leaves the composer as it was.
  await page.locator("#msg").fill("a draft in the composer");
  const sent = [];
  await page.route(`**/rooms/${ROOM}/messages`, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    const body = route.request().postDataJSON();
    sent.push(body);
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ id: 93006, room: ROOM, sender: body.sender, sender_type: "user", text: body.text,
        reply_to: body.reply_to, target: body.target, ts: Date.now() }),
    });
  });
  await panel.locator("#threadMsg").fill("I will write the rule up.");
  await panel.locator("#threadMsg").press("Enter");
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0].reply_to).toBe(93001);
  expect(sent[0].target).toBeNull();
  expect(sent[0].text).toBe("I will write the rule up.");
  expect(sent[0].op_id).toBeTruthy();
  await expect(panel.locator("#threadMsg")).toHaveValue("");
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveCount(4);
  await expect(link).toHaveText(/^4 replies · /);
  await expect(page.locator('#feed .row[data-id="93006"] .quote[data-goto="93001"]')).toHaveCount(1);
  await expect(page.locator("#msg")).toHaveValue("a draft in the composer");
  expect(await page.evaluate(() => state.replyTo)).toBeNull();

  // Closing gives the column back to This Loca.
  await panel.locator("#threadClose").click();
  await expect(panel).toBeHidden();
  await expect(page.locator("#locaInfo #sideLocaView")).toBeVisible();
  await page.close();
});

test("on a narrower screen the thread opens over the chat and Escape closes it", async ({ browser, request }) => {
  const ROOM = "threads-narrow";
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request, ROOM, { width: 1100, height: 820 });
  await seed(page, ROOM, [
    { id: 93101, sender: "deniz", text: "who has the release notes?" },
    { id: 93102, sender: "lead", sender_type: "agent", text: "I do.", reply_to: 93101 },
  ]);
  const link = page.locator('#feed .row[data-id="93101"] .threadlink');
  await expect(link).toHaveText(/^1 reply · last at /);
  await link.click();
  const panel = page.locator("#threadPanel");
  await expect(panel).toBeVisible();
  await expect(panel).toHaveClass(/overlay/);
  await expect(page.locator("#threadBackdrop")).toBeVisible();
  await expect(page.locator("#locaInfo #threadPanel")).toHaveCount(0);
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveCount(1);
  const box = await panel.boundingBox();
  expect(box.x + box.width).toBeLessThanOrEqual(1101);

  await panel.locator("#threadMsg").press("Escape");
  await expect(panel).toBeHidden();
  await expect(page.locator("#threadBackdrop")).toBeHidden();

  // Another tab puts the thread away; it is the chat's.
  await link.click();
  await expect(panel).toBeVisible();
  await page.evaluate(() => switchTab("notes"));
  await expect(panel).toBeHidden();
  await page.close();
});

test("a thread whose first message is further back reads it from the loca's history", async ({ browser, request }) => {
  const ROOM = "threads-earlier";
  await provisionRooms(request, [ROOM]);
  const page = await masterPage(browser, request, ROOM, { width: 1500, height: 900 });
  // A fresh loca: the seeded reply is the oldest message the chat holds.
  expect(await page.evaluate(() => state.seen.size)).toBe(0);
  await seed(page, ROOM, [
    { id: 93203, sender: "omer", text: "following up on this", reply_to: 93200 },
  ]);
  const quote = page.locator('#feed .row[data-id="93203"] .quote');
  await expect(quote).toHaveClass(/earlier/);
  await expect(quote).toHaveText("↩ replying to an earlier message");
  await expect(quote).toHaveAttribute("data-thread", "93200");

  // The first read fails, the second answers with the page after 93199.
  let reads = 0;
  await page.route(url => url.pathname === `/rooms/${ROOM}/messages` && url.searchParams.get("since") === "93199", async (route) => {
    reads += 1;
    if (reads === 1) return route.fulfill({ status: 503, body: "busy" });
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        { id: 93200, room: ROOM, sender: "reviewer", sender_type: "agent", text: "the first word of the thread", ts: Date.now() - 60_000 },
        { id: 93201, room: ROOM, sender: "deniz", sender_type: "user", text: "not part of it", ts: Date.now() - 50_000 },
        { id: 93202, room: ROOM, sender: "lead", sender_type: "agent", text: "an earlier reply", reply_to: 93200, ts: Date.now() - 40_000 },
      ]),
    });
  });

  await quote.click();
  const panel = page.locator("#threadPanel");
  await expect(panel).toBeVisible();
  await expect(panel.locator("#threadEarlierText")).toContainText("The start of this thread is further back.");
  await expect(panel.locator("#threadLoadEarlier")).toHaveText("Try again");
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveCount(1);

  await panel.locator("#threadLoadEarlier").click();
  await expect(panel.locator("#threadRoot")).toContainText("the first word of the thread");
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveText([/an earlier reply/, /following up on this/]);
  await expect(panel.locator("#threadEarlier")).toBeHidden();
  await expect(panel).not.toContainText("not part of it");
  expect(reads).toBe(2);
  // The quote in the chat now says what it answers, and still opens the thread.
  await expect(quote).not.toHaveClass(/earlier/);
  await expect(quote).toHaveText("↩ reviewer: the first word of the thread");

  // An answer from the box goes to the thread's first message.
  const sent = [];
  await page.route(`**/rooms/${ROOM}/messages`, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    sent.push(route.request().postDataJSON());
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({
      id: 93204, room: ROOM, sender: "x", sender_type: "user", text: sent[0].text, reply_to: sent[0].reply_to, ts: Date.now(),
    }) });
  });
  await panel.locator("#threadMsg").fill("noted");
  await panel.locator("#threadSend").click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0].reply_to).toBe(93200);
  await expect(panel.locator("#threadReplies .threadmsg")).toHaveCount(3);
  await page.close();
});
