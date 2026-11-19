const { defineConfig } = require("@playwright/test");
const port = process.env.PLAYWRIGHT_PORT || "18787";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "loca-browser-"));

module.exports = defineConfig({
  testDir: "tests/browser",
  timeout: 30_000,
  // An expectation waits up to 10 s for the page to reach what it checks.
  // The default 5 s was too tight under the parallel gate and a loaded CI
  // runner: lines written after the first server round trip (the door's
  // sign-in line) arrived after it. The tests still wait for the real state.
  expect: { timeout: 10_000 },
  workers: 1,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : {},
    trace: "retain-on-failure",
  },
  webServer: {
    command: [
      "env",
      `PORT=${port}`,
      "BIND_ADDR=127.0.0.1",
      "ADMIN_TOKEN=MASTER",
      `DB_PATH=${path.join(dataDir, "loca.sqlite3")}`,
      "ROOM_TOKEN=building",
      "REQUIRE_SESSIONS=1",
      "LEGACY_WS_QUERY_AUTH=0",
      // One server for the whole suite: its notification senders must not
      // meet the default six-per-10-minutes limit (the 429 test fulfils it).
      "NOTIFICATION_RATE_LIMIT=1000",
      "cargo run --quiet -p server",
    ].join(" "),
    url: `http://127.0.0.1:${port}/health`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
