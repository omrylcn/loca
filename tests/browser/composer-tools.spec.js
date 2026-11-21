const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Writing in the composer: a line break typed in chat stays a line break,
// as in any chat; emoji and Markdown formatting are a click or a key away,
// and Shift+Enter carries a list on. Notes keep Markdown's joined lines.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "composer-tools";

test.beforeEach(async ({ page, request }) => {
  await provisionRooms(request, [ROOM]);
  await page.addInitScript(() => { try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {} });
});

async function seated(page, request, name) {
  await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  const { token } = await (await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name, kind: "user" } })).json();
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.evaluate((room) => window.doConnect(room), ROOM);
  await expect(page.locator("#curRoom")).toHaveText(ROOM);
}

test("chat keeps the writer's line breaks; Notes still join a paragraph", async ({ page }) => {
  await page.goto("/");
  const chat = await page.evaluate(() => renderChatMarkdown("hediye kredi\naylık kredi\nfree tier"));
  expect(chat).toBe("<p>hediye kredi<br>aylık kredi<br>free tier</p>");
  const note = await page.evaluate(() => renderMarkdown("one\ntwo"));
  expect(note).toBe("<p>one two</p>");
});

test("emoji, formatting buttons, keys and list continuation write Markdown", async ({ page, request }) => {
  const name = `cw${Date.now().toString(36)}`;
  await seated(page, request, name);
  const msg = page.locator("#msg");

  // Emoji: the panel opens above the composer and puts one at the caret.
  await msg.fill("ship it ");
  await page.locator("#emojiBtn").click();
  await expect(page.locator("#emojiPop")).toBeVisible();
  await expect(page.locator("#emojiBtn")).toHaveAttribute("aria-expanded", "true");
  await page.locator('#emojiPop [data-emoji="🚀"]').click();
  await expect(msg).toHaveValue("ship it 🚀");
  await expect(page.locator("#emojiPop")).toBeHidden();

  // Bold around a selection, and off again.
  await msg.fill("make this bold");
  await msg.evaluate(t => t.setSelectionRange(10, 14));
  await page.locator("#formatBtn").click();
  await page.locator('#formatPop [data-format="bold"]').click();
  await expect(msg).toHaveValue("make this **bold**");
  await page.locator('#formatPop [data-format="bold"]').click();
  await expect(msg).toHaveValue("make this bold");

  // A list over the selected lines; Escape closes the panel.
  await msg.fill("one\ntwo");
  await msg.evaluate(t => t.setSelectionRange(0, t.value.length));
  await page.locator('#formatPop [data-format="number"]').click();
  await expect(msg).toHaveValue("1. one\n2. two");
  await page.keyboard.press("Escape");
  await expect(page.locator("#formatPop")).toBeHidden();

  // Keys: Ctrl+I and Ctrl+K with nothing selected leave a placeholder.
  await msg.fill("");
  await msg.focus();
  await page.keyboard.press("Control+i");
  await expect(msg).toHaveValue("*italic*");
  await msg.fill("see ");
  await msg.evaluate(t => t.setSelectionRange(4, 4));
  await page.keyboard.press("Control+k");
  await expect(msg).toHaveValue("see [link](https://)");

  // Shift+Enter carries a list on; on an empty item it ends the list.
  await msg.fill("- milk");
  await msg.evaluate(t => t.setSelectionRange(t.value.length, t.value.length));
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("eggs");
  await expect(msg).toHaveValue("- milk\n- eggs");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.press("Shift+Enter");
  await expect(msg).toHaveValue("- milk\n- eggs\n");

  // Sent, the lines stay apart in the feed.
  await msg.fill("first line\nsecond line");
  await page.locator("#sendBtn").click();
  await expect(page.locator("#feed .row[data-id] .chatmarkdown p", { hasText: "first line" }).locator("br")).toHaveCount(1);
});
