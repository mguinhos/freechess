import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT, PEER_PORT, openApp, gameId, joinGame, expectSeated, playMove,
} from "./helpers";

test("Black creator, promotions, replay, resignation and PGN work across nodes", async ({ browser }) => {
  const black = await openApp(browser, GATEWAY_PORT, "black-player");
  const white = await openApp(browser, PEER_PORT, "white-player");
  const blackBefore = Number(await black.app.locator("#account-button .muted").innerText());
  const whiteBefore = Number(await white.app.locator("#account-button .muted").innerText());
  await black.app.getByRole("button", { name: "New game", exact: true }).click();
  const modal = black.app.locator(".modal");
  await modal.getByRole("button", { name: "Black", exact: true }).click();
  await modal.getByRole("button", { name: "Create game", exact: true }).click();
  await expect(black.app.locator(".board > .sq").first()).toHaveAttribute("data-square", "h1");
  await expect(black.app.locator(".seat").last()).toHaveAttribute("data-color", "black");
  await expect(black.app.getByText("You are playing this game.", { exact: true })).toBeVisible();
  const id = await gameId(black.app);
  const whiteGame = await joinGame(white.page, PEER_PORT, id);
  await expectSeated(black.app, "white-player");

  const moves = ["a2a4", "h7h5", "a4a5", "h5h4", "a5a6", "h4h3", "a6b7", "h3g2"];
  for (const [i, uci] of moves.entries()) {
    await playMove(i % 2 === 0 ? whiteGame : black.app, uci.slice(0, 2), uci.slice(2));
    await expect(whiteGame.locator(".moves .mv")).toHaveCount(i + 1);
    await expect(black.app.locator(".moves .mv")).toHaveCount(i + 1);
  }
  await playMove(whiteGame, "b7", "a8");
  await expect(whiteGame.getByRole("dialog", { name: "Choose a promotion piece" })).toBeVisible();
  await expect(whiteGame.locator(".moves .mv")).toHaveCount(8);
  await whiteGame.getByRole("button", { name: "Knight", exact: true }).click();
  await expect(black.app.locator('.sq[data-square="a8"]')).toHaveAttribute("data-piece", "N");
  await playMove(black.app, "g2", "h1");
  await black.app.getByRole("button", { name: "Queen", exact: true }).click();
  await expect(whiteGame.locator(".moves .mv")).toHaveCount(10);
  await expect(whiteGame.locator('.sq[data-square="h1"]')).toHaveAttribute("data-piece", "q");

  await black.app.locator("#flip-board").click();
  await expect(black.app.locator(".board > .sq").first()).toHaveAttribute("data-square", "a8");
  await expect(black.app.locator(".seat").last()).toHaveAttribute("data-color", "white");
  await black.app.getByRole("button", { name: "Starting position", exact: true }).click();
  await expect(black.app.locator('.sq[data-square="a8"]')).toHaveAttribute("data-piece", "r");
  await black.app.getByRole("button", { name: "Latest position", exact: true }).click();
  await black.app.getByRole("button", { name: "Resign", exact: true }).click();
  await black.app.getByRole("button", { name: "Resign game", exact: true }).click();
  await expect(whiteGame.locator(".result-card")).toContainText("You won");
  await whiteGame.getByRole("button", { name: "Review game", exact: true }).click();
  await whiteGame.locator("#export-pgn").click();
  await expect(whiteGame.locator("#pgn-text")).toHaveValue(/bxa8=N.*gxh1=Q.*1-0/s);
  const downloaded = white.page.waitForEvent("download");
  await whiteGame.getByRole("link", { name: "Download", exact: true }).click();
  expect((await downloaded).suggestedFilename()).toBe(`freechess-${id}.pgn`);
  await expect.poll(async () => Number(await black.app.locator("#account-button .muted").innerText())).toBeLessThan(blackBefore);
  await expect.poll(async () => Number(await whiteGame.locator("#account-button .muted").innerText())).toBeGreaterThan(whiteBefore);
  await black.context.close();
  await white.context.close();
});
