const { test, expect } = require("@playwright/test");

// The redesign themes name their sans face instead of leaving it to
// system-ui: a Linux machine's system-ui is DejaVu Sans on one and a CJK
// face on another, and those widths decided whether a row of buttons
// fitted (People's actions, the seats at the table). Liberation Sans is
// there wherever the browser tests run, so local and CI measure alike.
test("the Loca, Calm and Console themes name their sans face", async ({ page }) => {
  await page.goto("/");
  for (const theme of ["loca", "calm", "console"]) {
    const family = await page.evaluate((t) => {
      applyAppearance({ theme: t, font_family: "sans" });
      return getComputedStyle(document.body).fontFamily;
    }, theme);
    expect(family, theme).not.toMatch(/^\s*system-ui/);
    expect(family, theme).toContain("Liberation Sans");
  }
});
