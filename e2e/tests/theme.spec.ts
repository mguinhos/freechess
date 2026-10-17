import { test, expect, FrameLocator, Locator, Page } from "@playwright/test";
import { app, appUrl, GATEWAY_PORT, waitForAccount } from "./helpers";

const lightSquare = '.board > .sq[data-square="a8"]';
const darkSquare = '.board > .sq[data-square="b8"]';
const whitePiece = '.board > .sq[data-square="e2"] svg';
const blackPiece = '.board > .sq[data-square="e7"] svg';

function rgb(hex: string): string {
  return `rgb(${[1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)).join(", ")})`;
}

async function practice(page: Page): Promise<FrameLocator> {
  await page.goto(appUrl(GATEWAY_PORT), { waitUntil: "domcontentloaded" });
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  await expect(frame.locator(".board > .sq")).toHaveCount(64);
  await expect(frame.locator("#nickname-prompt")).toBeHidden();
  await waitForAccount(frame);
  await expect(frame.locator("#theme-button")).toHaveAttribute("data-loaded", "true");
  return frame;
}

async function appearance(frame: FrameLocator): Promise<Locator> {
  await frame.locator("#theme-button").click();
  const dialog = frame.getByRole("dialog", { name: "Appearance", exact: true });
  await expect(dialog).toBeVisible();
  return dialog;
}

async function color(input: Locator, value: string) {
  // Native color pickers are outside the page; dispatch the same input event.
  await input.evaluate((element, value) => {
    const input = element as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, value);
  await expect(input).toHaveValue(value);
}

async function expectPieceFill(piece: Locator, hex: string) {
  await expect.poll(() => piece.evaluate((element, expected) =>
    [...element.querySelectorAll("path,circle,ellipse,polygon,rect")]
      .some(shape => getComputedStyle(shape).fill === expected), rgb(hex)),
  ).toBe(true);
}

test("appearance presets, piece sets and custom colors update the actual board", async ({ page }) => {
  const frame = await practice(page);
  const dialog = await appearance(frame);
  const historyLength = await page.evaluate(() => history.length);
  await dialog.getByRole("button", { name: "Reset defaults", exact: true }).click();
  await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
  await expect(dialog.getByRole("button", { name: "Neo", exact: true })).toHaveAttribute("aria-pressed", "true");
  const neo = await frame.locator(whitePiece).innerHTML();
  const defaultLight = await dialog.locator("#light-square-color").inputValue();
  const defaultDark = await dialog.locator("#dark-square-color").inputValue();

  for (const name of ["Blue", "Brown"]) {
    await dialog.getByRole("button", { name, exact: true }).click();
    await expect(dialog.getByRole("button", { name, exact: true })).toHaveAttribute("aria-pressed", "true");
    const light = await dialog.locator("#light-square-color").inputValue();
    const dark = await dialog.locator("#dark-square-color").inputValue();
    expect(dark).not.toBe(defaultDark);
    await expect(frame.locator(lightSquare)).toHaveCSS("background-color", rgb(light));
    await expect(frame.locator(darkSquare)).toHaveCSS("background-color", rgb(dark));
  }
  await dialog.getByRole("button", { name: "Classic", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Classic", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.locator(whitePiece).innerHTML()).not.toBe(neo);

  await color(dialog.locator("#light-square-color"), "#123456");
  await color(dialog.locator("#dark-square-color"), "#654321");
  await color(dialog.locator("#white-piece-color"), "#abcdef");
  await color(dialog.locator("#black-piece-color"), "#fedcba");
  await expect(frame.locator(lightSquare)).toHaveCSS("background-color", "rgb(18, 52, 86)");
  await expect(frame.locator(darkSquare)).toHaveCSS("background-color", "rgb(101, 67, 33)");
  await expectPieceFill(frame.locator(whitePiece), "#abcdef");
  await expectPieceFill(frame.locator(blackPiece), "#fedcba");
  await dialog.getByRole("button", { name: "Neo", exact: true }).click();
  await expectPieceFill(frame.locator(whitePiece), "#abcdef");
  await expectPieceFill(frame.locator(blackPiece), "#fedcba");

  await dialog.getByRole("button", { name: "Reset defaults", exact: true }).click();
  await expect(dialog.locator("#light-square-color")).toHaveValue(defaultLight);
  await expect(dialog.locator("#dark-square-color")).toHaveValue(defaultDark);
  await expect(dialog.getByRole("button", { name: "Neo", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => frame.locator(whitePiece).innerHTML()).toBe(neo);
  await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
  await expect.poll(() => page.evaluate(() => history.length)).toBe(historyLength);
  await expect(page).toHaveURL(appUrl(GATEWAY_PORT));
});

test("account appearance and reset survive fresh browser contexts from the bare app URL", async ({ browser }) => {
  let context = await browser.newContext();
  try {
    let page = await context.newPage();
    let frame = await practice(page);
    let dialog = await appearance(frame);
    await dialog.getByRole("button", { name: "Classic", exact: true }).click();
    await color(dialog.locator("#light-square-color"), "#112233");
    await color(dialog.locator("#dark-square-color"), "#445566");
    await color(dialog.locator("#white-piece-color"), "#ddeeff");
    await color(dialog.locator("#black-piece-color"), "#223344");
    await expectPieceFill(frame.locator(whitePiece), "#ddeeff");
    await expectPieceFill(frame.locator(blackPiece), "#223344");
    await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
    await expect(page).toHaveURL(appUrl(GATEWAY_PORT));
    expect(await frame.locator("body").evaluate(() => location.hash)).toBe("");

    await context.close();
    context = await browser.newContext();
    page = await context.newPage();
    frame = await practice(page);
    dialog = await appearance(frame);
    await expect(dialog.getByRole("button", { name: "Classic", exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(dialog.locator("#light-square-color")).toHaveValue("#112233");
    await expect(dialog.locator("#dark-square-color")).toHaveValue("#445566");
    await expect(dialog.locator("#white-piece-color")).toHaveValue("#ddeeff");
    await expect(dialog.locator("#black-piece-color")).toHaveValue("#223344");
    await expectPieceFill(frame.locator(whitePiece), "#ddeeff");
    await expectPieceFill(frame.locator(blackPiece), "#223344");
    await expect(frame.locator(lightSquare)).toHaveCSS("background-color", "rgb(17, 34, 51)");
    await expect(frame.locator(darkSquare)).toHaveCSS("background-color", "rgb(68, 85, 102)");
    await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
    await expect(page).toHaveURL(appUrl(GATEWAY_PORT));

    const controls = dialog.locator('button, input, select, textarea, a[href], [tabindex="0"]');
    const first = controls.first();
    const last = controls.last();
    await expect(first).toBeFocused();
    await first.press("Shift+Tab");
    await expect(last).toBeFocused();
    await last.press("Tab");
    await expect(first).toBeFocused();
    await dialog.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(frame.locator("#theme-button")).toBeFocused();

    dialog = await appearance(frame);
    await dialog.getByRole("button", { name: "Reset defaults", exact: true }).click();
    await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
    await context.close();

    context = await browser.newContext();
    page = await context.newPage();
    frame = await practice(page);
    dialog = await appearance(frame);
    await expect(dialog.getByRole("button", { name: "Neo", exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(dialog.locator("#light-square-color")).toHaveValue("#ebecd0");
    await expect(dialog.locator("#dark-square-color")).toHaveValue("#739552");
    await expect(dialog.locator("#white-piece-color")).toHaveValue("#f9f9f9");
    await expect(dialog.locator("#black-piece-color")).toHaveValue("#555555");
    await expect(frame.locator(lightSquare)).toHaveCSS("background-color", rgb("#ebecd0"));
    await expect(frame.locator(darkSquare)).toHaveCSS("background-color", rgb("#739552"));
    await expectPieceFill(frame.locator(whitePiece), "#f9f9f9");
    await expectPieceFill(frame.locator(blackPiece), "#555555");
    await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
    await expect(page).toHaveURL(appUrl(GATEWAY_PORT));
    expect(await frame.locator("body").evaluate(() => location.hash)).toBe("");
  } finally {
    await context.close();
  }
});
