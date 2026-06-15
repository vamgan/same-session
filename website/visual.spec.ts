import { expect, test } from "@playwright/test";

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test(`${viewport.name} product page`, async ({ page }) => {
    await page.setViewportSize(viewport);
    const errors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(error.message));

    await page.goto("http://127.0.0.1:5173/same-session/");
    for (const [name, selector] of [
      ["protocol", "#protocol"],
      ["commands", ".commands"],
      ["security", "#security"],
      ["final", ".final"],
    ]) {
      await page.locator(selector).scrollIntoViewIfNeeded();
      await page.waitForTimeout(700);
      await page.screenshot({ path: `/tmp/samesession-${viewport.name}-${name}.png` });
    }

    await page.locator(".commands").scrollIntoViewIfNeeded();
    await page.locator(".copy-command").first().click();
    await expect(page.locator(".copy-command").first()).toContainText(/Copied|Copy unavailable/);
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
    expect(errors).toEqual([]);
  });
}
