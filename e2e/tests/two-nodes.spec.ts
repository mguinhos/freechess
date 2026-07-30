import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT,
  PEER_PORT,
  PEER2_PORT,
  createGame,
  expectCleanConsole,
  expectSeated,
  joinGame,
  gameId,
  moveList,
  openApp,
  playMove,
  reopen,
} from "./helpers";

/**
 * These tests are the point of the whole exercise: they check that the app
 * works *across two independent Freenet nodes*, not just within one browser.
 * Anything that passes on a single node proves nothing about replication.
 */

test("the app loads on both nodes with a clean console", async ({ browser }) => {
  for (const port of [GATEWAY_PORT, PEER_PORT]) {
    const s = await openApp(browser, port);
    await expect(s.app.locator(".brand")).toContainText("FreeChess");
    // The stylesheet has to survive the gateway's same-origin CSP; if it were
    // blocked, the top bar would fall back to a transparent background.
    const bg = await s.app
      .locator(".topbar")
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg, "stylesheet did not load").not.toBe("rgba(0, 0, 0, 0)");
    expectCleanConsole(s.errors);
    await s.context.close();
  }
});

test("a game created on one node appears on the other", async ({ browser }) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await createGame(alice.app, "10+0");

  // Bob's home page should list Alice's open challenge — this is the lobby
  // contract replicating from one node to the other. Addressed by id: matching
  // on "alice" would be satisfied by any leftover game of hers and could never
  // fail.
  const id = await gameId(alice.app);
  const bobHome = await reopen(bob.page, PEER_PORT);
  await expect(bobHome.locator(`#listing-${id}`)).toBeVisible();

  expectCleanConsole(alice.errors);
  expectCleanConsole(bob.errors);
  await alice.context.close();
  await bob.context.close();
});

test("two players on different nodes play a game to checkmate", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  // Alice creates and plays White.
  await createGame(alice.app, "10+0");
  // Reading the share link also verifies the app produced a usable one.
  const id = await gameId(alice.app);

  // Bob joins from the other node, and Alice's client countersigns him before
  // any move is legal.
  const bobHome = await joinGame(bob.page, PEER_PORT, id);
  await expectSeated(alice.app, "bob");

  // Fool's mate: Black wins on move 2.
  await playMove(alice.app, "f2", "f3");
  await expect
    .poll(() => moveList(bobHome), { message: "Bob never saw f3" })
    .toContain("f3");

  await playMove(bobHome, "e7", "e5", "black");
  await expect.poll(() => moveList(alice.app)).toContain("e5");

  await playMove(alice.app, "g2", "g4");
  await expect.poll(() => moveList(bobHome)).toContain("g4");

  await playMove(bobHome, "d8", "h4", "black");

  // Both sides must independently reach the same verdict — the result is
  // derived from the move list by each client's own engine, not announced by
  // one of them.
  await expect(alice.app.locator(".result-card")).toContainText("You lost");
  await expect(bobHome.locator(".result-card")).toContainText("You won");
  await expect.poll(() => moveList(alice.app)).toContain("Qh4#");

  // Base58 of 32 bytes is 43-44 chars depending on leading zeros, so match the
  // shape rather than a fixed length.
  expect(id).toMatch(/^[1-9A-HJ-NP-Za-km-z]{43,44}$/);
  await alice.context.close();
  await bob.context.close();
});

test("a spectator sees the game live and cannot move", async ({ browser }) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);

  const bobHome = await joinGame(bob.page, PEER_PORT, id);
  await expectSeated(alice.app, "bob");

  // A third party opens the same game by URL — from a THIRD node, because a
  // second context on Bob's node would be Bob himself.
  const carol = await openApp(browser, PEER2_PORT, "carol");
  const carolGame = await reopen(carol.page, PEER2_PORT, `?game=${id}`);
  await expect(carolGame.locator(".board")).toBeVisible();
  await expect(carolGame.getByText("You are spectating")).toBeVisible();

  // Alice moves; Carol sees it without being a player.
  await playMove(alice.app, "e2", "e4");
  await expect.poll(() => moveList(carolGame)).toContain("e4");

  // Carol tries to move Black's pawn. The board offers her nothing, and the
  // move list must not grow — the contract would reject her signature anyway,
  // but the UI should not even attempt it.
  const before = await moveList(carolGame);
  await playMove(carolGame, "e7", "e5", "white");
  await carol.page.waitForTimeout(3000);
  expect(await moveList(carolGame)).toEqual(before);

  await alice.context.close();
  await bob.context.close();
  await carol.context.close();
});

test("a game link replays for someone who never saw it live", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);

  const bobHome = await joinGame(bob.page, PEER_PORT, id);
  await expectSeated(alice.app, "bob");

  await playMove(alice.app, "e2", "e4");
  await expect.poll(() => moveList(bobHome)).toContain("e4");
  await playMove(bobHome, "e7", "e5", "black");
  await expect.poll(() => moveList(alice.app)).toContain("e5");
  await playMove(alice.app, "g1", "f3");
  await expect.poll(() => moveList(bobHome)).toContain("Nf3");

  // A brand-new visitor opens the shared link from a third node, so they are
  // genuinely someone else and genuinely never saw the game live.
  const viewer = await openApp(browser, PEER2_PORT);
  const replay = await reopen(viewer.page, PEER2_PORT, `?game=${id}`);
  await expect(replay.locator(".board")).toBeVisible();
  await expect.poll(() => moveList(replay)).toEqual(["e4", "e5", "Nf3"]);

  // Scrub back to the start: the replay controls must reconstruct the opening
  // position from the move list alone.
  await replay.locator(".replay-bar button").first().click();
  await expect(replay.locator("#replay-position")).toContainText("Viewing move 0");

  await alice.context.close();
  await bob.context.close();
  await viewer.context.close();
});

test("the anti-spam quota stops a flood of open games", async ({ browser }) => {
  const spammer = await openApp(browser, GATEWAY_PORT, "spammer");

  // Create more open games than the per-creator quota (3) allows.
  for (let i = 0; i < 5; i++) {
    const home = await reopen(spammer.page, GATEWAY_PORT);
    await createGame(home, "10+0");
  }

  const home = await reopen(spammer.page, GATEWAY_PORT);
  // The lobby converges to at most the quota; the rest are evicted
  // deterministically.
  await expect
    .poll(
      async () => home.locator(".list-item", { hasText: "spammer" }).count(),
      { message: "quota was not enforced" },
    )
    .toBeLessThanOrEqual(3);

  await spammer.context.close();
});
