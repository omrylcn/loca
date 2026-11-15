// The browser accounts for every server frame in docs/protocol-frames.json:
// onFrame dispatches on it, or WEB_FRAME_HANDLING records why it is ignored.
// It reads the table and onFrame from the page the server actually serves.
const fs = require("node:fs");
const path = require("node:path");
const { test, expect } = require("@playwright/test");

const ROOT = path.join(__dirname, "..", "..");
const LIST = JSON.parse(fs.readFileSync(path.join(ROOT, "docs/protocol-frames.json"), "utf8"));

test("every server frame is handled or deliberately ignored", async ({ page }) => {
  await page.goto("/");
  const { table, source } = await page.evaluate(() => ({
    table: WEB_FRAME_HANDLING,
    source: onFrame.toString(),
  }));
  // Every frame the server sends; a frame the server does not send yet may be
  // listed only if docs/protocol-frames.json names it under not_sent_yet.
  const server = Object.keys(LIST.server_frames).sort();
  expect(Object.keys(table).filter((tag) => tag in LIST.server_frames).sort()).toEqual(server);
  for (const tag of Object.keys(table)) {
    if (!(tag in LIST.server_frames)) expect(Object.keys(LIST.not_sent_yet), tag).toContain(tag);
  }
  for (const [tag, decision] of Object.entries(table)) {
    if (decision === "handled") continue;
    expect(decision, tag).toMatch(/^ignored: .{10,}/);
  }
  const dispatched = new Set([...source.matchAll(/\bf\.t === "([a-z_]+)"/g)].map((m) => m[1]));
  const handled = Object.keys(table).filter((tag) => table[tag] === "handled");
  for (const tag of handled) expect(dispatched.has(tag), `onFrame never compares ${tag}`).toBe(true);
  for (const tag of dispatched) expect(tag in table, `onFrame dispatches on unlisted ${tag}`).toBe(true);
});

test("the browser sends only listed client frames", () => {
  const sent = new Set();
  for (const file of fs.readdirSync(path.join(ROOT, "web/assets"))) {
    if (!file.endsWith(".js")) continue;
    const source = fs.readFileSync(path.join(ROOT, "web/assets", file), "utf8");
    for (const m of source.matchAll(/\.send\(JSON\.stringify\(\{\s*t:\s*"([a-z_]+)"/g)) sent.add(m[1]);
  }
  expect(sent.size).toBeGreaterThan(0);
  for (const tag of sent) expect(Object.keys(LIST.client_frames), tag).toContain(tag);
});
