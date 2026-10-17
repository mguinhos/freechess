import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT, PEER_PORT, openApp, createGame, gameId,
  joinGame, expectSeated, playMove,
} from "./helpers";

test("returning to current position resumes following own and incoming moves", async ({ browser }) => {
  const white = await openApp(browser, GATEWAY_PORT, "replay-white");
  const black = await openApp(browser, PEER_PORT, "replay-black");
  try {
    await createGame(white.app);
    const id = await gameId(white.app);
    const blackGame = await joinGame(black.page, PEER_PORT, id);
    await expectSeated(white.app, "replay-black");
    await playMove(white.app, "e2", "e4");
    await expect(blackGame.locator(".moves .mv")).toHaveCount(1);
    await playMove(blackGame, "e7", "e5");
    await expect(white.app.locator(".moves .mv")).toHaveCount(2);

    await white.app.getByRole("button", { name: "Starting position", exact: true }).click();
    await expect(white.app.locator("#replay-position")).toContainText("Viewing position 0 of 2");
    const next = white.app.getByRole("button", { name: "Next move", exact: true });
    await next.click(); await next.click();
    await expect(white.app.locator("#replay-position")).toBeHidden();
    await playMove(white.app, "g1", "f3");
    await expect(white.app.locator(".moves .mv")).toHaveCount(3);
    await expect(white.app.locator('.board > .sq[data-square="f3"]')).toHaveAttribute("data-piece", "N");
    await expect(blackGame.locator(".moves .mv")).toHaveCount(3);
    await playMove(blackGame, "b8", "c6");
    await expect(white.app.locator(".moves .mv")).toHaveCount(4);
    await expect(white.app.locator('.board > .sq[data-square="c6"]')).toHaveAttribute("data-piece", "n");
    await expect(white.app.locator("#replay-position")).toBeHidden();

    // Clicking the latest SAN move must also resume live updates.
    await white.app.getByRole("button", { name: "Starting position", exact: true }).click();
    await white.app.locator(".moves .mv").last().click();
    await playMove(white.app, "f1", "c4");
    await expect(white.app.locator(".moves .mv")).toHaveCount(5);
    await expect(white.app.locator('.board > .sq[data-square="c4"]')).toHaveAttribute("data-piece", "B");
    await expect(white.app.locator("#replay-position")).toBeHidden();
  } finally {
    const resign = white.app.getByRole("button", { name: "Resign", exact: true });
    if (await resign.isVisible()) {
      await resign.click();
      await white.app.getByRole("button", { name: "Resign game", exact: true }).click();
    }
    await white.context.close();
    await black.context.close();
  }
});
