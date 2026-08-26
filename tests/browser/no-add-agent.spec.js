const { test, expect } = require("@playwright/test");

// The persistent "Add agent" affordance (where the Master live-minted a davet)
// was removed in favour of the agent-initiated join-request model. It lived
// only as a desktop-injected shim (HOST_SHIM -> a floating "#_lc_add" button +
// "#_lc_box"/"#_lc_modal" panel); the shared web UI never carried it. The
// desktop is a no-fork wrapper on THIS same UI, so proving the affordance is
// absent here proves it for both web and desktop — same logic, both surfaces.

test("no Add-agent affordance in the shared web UI", async ({ page }) => {
  await page.goto("/"); // waits for load; a reintroduced on-load shim would show
  for (const sel of ["#_lc_add", "#_lc_box", "#_lc_modal"]) {
    await expect(page.locator(sel)).toHaveCount(0);
  }
  await expect(page.getByRole("button", { name: /add agent/i })).toHaveCount(0);
});

test("Host mode does not reintroduce an Add-agent affordance", async ({ page }) => {
  // The removed shim gated on window.__LOCA_HOST__. No web code renders Add-agent
  // from that flag, and the desktop no longer injects a shim, so setting it must
  // still leave the affordance absent.
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");
  await expect(page.locator("#_lc_add")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /add agent/i })).toHaveCount(0);
});
