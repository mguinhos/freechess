import { test, expect, FrameLocator, Page } from "@playwright/test";
import { app, appUrl, waitForAccount } from "./helpers";

const port = Number(process.env.FREECHESS_THEME_ACCOUNT_PORT);

// Public disposable fixtures from seeds [201; 32] and [202; 32], checked by account::import/export.
const alice = {
  key: "fchess12XsQm36Z8rLuZjGBjstphbEZzkrCykz6GaGA7AcXSBYNq8Th8q",
  id: "Eo4XrY1fszDqk6MKuppQe6",
};
const bob = {
  key: "fchess12YK46ssbc1phzi2hWpPwHZ6ZJ4xWe8ZGgbsSK2agXnoFQpRwmQ",
  id: "3aDs19N7PH6Z12XTse1DZe",
};
const blue = {
  pieces: "Classic",
  colors: {
    "light-square-color": "#112233", "dark-square-color": "#445566",
    "white-piece-color": "#ddeeff", "black-piece-color": "#223344",
  },
};
const brown = {
  pieces: "Neo",
  colors: {
    "light-square-color": "#f0d9b5", "dark-square-color": "#b58863",
    "white-piece-color": "#fff4dd", "black-piece-color": "#4a2612",
  },
};

async function ready(page: Page): Promise<FrameLocator> {
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  await waitForAccount(frame);
  await expect(frame.locator("#theme-button")).toHaveAttribute("data-loaded", "true");
  return frame;
}

async function importAccount(page: Page, frame: FrameLocator, account: typeof alice) {
  await frame.locator("#account-button").click();
  const dialog = frame.getByRole("dialog", { name: "Your account", exact: true });
  await dialog.locator("#account-import").fill(account.key);
  await Promise.all([
    page.waitForEvent("framenavigated", {
      predicate: frame => frame.parentFrame() === page.mainFrame(),
      timeout: 20_000,
    }),
    dialog.getByRole("button", { name: "Import and reload", exact: true }).click(),
  ]);
  frame = await ready(page);
  await frame.locator("#account-button").click();
  const imported = frame.getByRole("dialog", { name: "Your account", exact: true });
  await expect(imported.locator(".mono")).toHaveText(account.id);
  await imported.press("Escape");
  return frame;
}

async function appearance(frame: FrameLocator) {
  await frame.locator("#theme-button").click();
  const dialog = frame.getByRole("dialog", { name: "Appearance", exact: true });
  await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
  return dialog;
}

async function chooseTheme(frame: FrameLocator, theme: typeof blue) {
  const dialog = await appearance(frame);
  await dialog.getByRole("button", { name: theme.pieces, exact: true }).click();
  for (const [id, value] of Object.entries(theme.colors)) {
    await dialog.locator(`#${id}`).evaluate((element, value) => {
      const input = element as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }, value);
  }
  await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
  await dialog.press("Escape");
}

async function expectTheme(frame: FrameLocator, theme: typeof blue) {
  const dialog = await appearance(frame);
  await expect(dialog.getByRole("button", { name: theme.pieces, exact: true })).toHaveAttribute("aria-pressed", "true");
  for (const [id, value] of Object.entries(theme.colors)) {
    await expect(dialog.locator(`#${id}`)).toHaveValue(value);
  }
  await dialog.press("Escape");
}

test("appearance stays with each imported account on a disposable node", async ({ browser }) => {
  test.skip(!process.env.FREECHESS_THEME_ACCOUNT_PORT, "Requires an explicitly provisioned disposable node");
  expect(Number.isInteger(port) && port > 0 && port <= 65535).toBe(true);
  expect([7509, 7511, 7512, 7514], "Never import test accounts on regular user/test nodes").not.toContain(port);

  let context = await browser.newContext();
  try {
    let page = await context.newPage();
    await page.goto(appUrl(port), { waitUntil: "domcontentloaded" });
    let frame = await ready(page);
    frame = await importAccount(page, frame, alice);
    await chooseTheme(frame, blue);
    frame = await importAccount(page, frame, bob);
    await chooseTheme(frame, brown);
    frame = await importAccount(page, frame, alice);
    await expectTheme(frame, blue);
    frame = await importAccount(page, frame, bob);
    await expectTheme(frame, brown);
    await expect(page).toHaveURL(appUrl(port));
    await context.close();

    context = await browser.newContext();
    page = await context.newPage();
    await page.goto(appUrl(port), { waitUntil: "domcontentloaded" });
    frame = await ready(page);
    await expectTheme(frame, brown);
    frame = await importAccount(page, frame, alice);
    await expectTheme(frame, blue);
    await expect(page).toHaveURL(appUrl(port));
  } finally {
    await context.close();
  }
});
