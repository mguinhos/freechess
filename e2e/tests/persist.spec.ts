import { test, expect } from "@playwright/test";

const URL = "http://127.0.0.1:7513/v1/contract/web/HU6wnWdp5a9YPT9kKWYxJk9PBQKFkwEDTW4VNDConEP3/";

// The bug this guards: the gateway sandboxes the app without allow-same-origin,
// so localStorage throws and the account was regenerated on every load. The
// delegate is the only storage that survives, so identity must come from it.
test("the account survives a reload", async ({ page }) => {
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  let app = page.frameLocator("#app");
  await expect(app.locator(".conn .dot.online")).toBeVisible();
  await app.locator("#account-button").click();
  const first = await app.locator(".modal .mono").innerText();
  await page.keyboard.press("Escape");

  await page.reload({ waitUntil: "domcontentloaded" });
  app = page.frameLocator("#app");
  await expect(app.locator(".conn .dot.online")).toBeVisible();
  // Give the delegate round-trip a moment to land.
  await page.waitForTimeout(4000);
  await app.locator("#account-button").click();
  const second = await app.locator(".modal .mono").innerText();

  expect(second.trim()).toBe(first.trim());
});
