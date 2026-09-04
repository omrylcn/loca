const { defineConfig } = require("@playwright/test");
const port = process.env.PLAYWRIGHT_PORT || "18787";

module.exports = defineConfig({
  testDir: "tests/browser",
  timeout: 30_000,
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
      "DB_PATH=:memory:",
      "ROOM_TOKEN=building",
      "REQUIRE_SESSIONS=1",
      "LEGACY_WS_QUERY_AUTH=0",
      "cargo run --quiet -p server",
    ].join(" "),
    url: `http://127.0.0.1:${port}/health`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
