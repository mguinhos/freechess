import { test, expect } from "@playwright/test";
import { GATEWAY_PORT, appUrl } from "./helpers";

// The bug this guards: the gateway sandboxes the app without allow-same-origin,
// so localStorage throws and the account was regenerated on every load. The
// delegate is the only storage that survives, so identity must come from it.
//
// The URL comes from the helpers, which read the contract id off disk. It used
// to be hardcoded, along with a port that is not even part of the test network
// — so this test failed with ERR_CONNECTION_REFUSED and was written off as a
// flaky delegate round-trip for far too long.
test("the account survives a reload", async ({ page }) => {
  await page.goto(appUrl(GATEWAY_PORT), { waitUntil: "domcontentloaded" });
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
