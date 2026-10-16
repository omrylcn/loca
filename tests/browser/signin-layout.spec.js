const { test, expect } = require("@playwright/test");

for (const width of [390, 1280]) {
  test(`sign-in fields share theme and fit at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
    await page.goto("/");
    const kind = page.getByLabel("Sign in as", { exact: true });
    await expect(kind).toBeVisible();
    await kind.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(kind).toHaveValue("smaster");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Tab");
    await expect(page.locator("#name")).toBeFocused();
    await expect(page.locator("#name")).toHaveAttribute("placeholder", "optional — your key verifies your identity");
    await expect(page.locator("#joinRequestBtn")).toBeHidden();
    await expect(page.locator("#connectBtn")).toHaveText("sign in as Smaster");
    const sizes = await page.evaluate(() => {
      const selector = document.querySelector("#entryKind");
      const input = document.querySelector("#roomToken");
      return {
        select: selector.getBoundingClientRect().width,
        input: input.getBoundingClientRect().width,
        selectHeight: selector.getBoundingClientRect().height,
        themed: getComputedStyle(selector).backgroundColor === getComputedStyle(input).backgroundColor,
        fits: document.documentElement.scrollWidth <= innerWidth,
      };
    });
    expect(sizes.select).toBe(sizes.input);
    expect(sizes.selectHeight).toBeGreaterThanOrEqual(40);
    expect(sizes.themed).toBe(true);
    expect(sizes.fits).toBe(true);
    await testInfo.attach(`smaster-${width}`, { body: await page.screenshot(), contentType: "image/png" });
    await page.locator("#masterEntryButton").click();
    await expect(page.getByLabel("Master pairing code", { exact: true })).toBeVisible();
    await expect(kind).toBeHidden();
    await page.locator("#masterEntryButton").click();
    await expect(kind).toHaveValue("smaster");
    await expect(kind).toBeVisible();
  });
}
