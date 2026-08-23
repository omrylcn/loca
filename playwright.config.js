const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "tests/browser",
  timeout: 30_000,
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:18787",
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : {},
    trace: "retain-on-failure",
  },
  webServer: {
    command: [
      "env",
      "PORT=18787",
      "BIND_ADDR=127.0.0.1",
      "ADMIN_TOKEN=MASTER",
      "DB_PATH=:memory:",
      "ROOM_TOKEN=building",
      "REQUIRE_SESSIONS=1",
      "LEGACY_WS_QUERY_AUTH=0",
      "cargo run --quiet -p server",
    ].join(" "),
    url: "http://127.0.0.1:18787/health",
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
