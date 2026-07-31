import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT,
  PEER_PORT,
  createGame,
  expectSeated,
  gameId,
  joinGame,
  moveList,
  openApp,
  playMove,
  reopen,
} from "./helpers";

/**
 * The migration notice: telling everyone the app has moved.
 *
 * **The filename forces this to run last.** Playwright orders by path, and an
 * announced migration locks the lobby against new listings — so if this ran
 * earlier, every later test would fail to create a game. It cancels the notice
 * at the end, but a failure part-way through would leave the lobby locked, and
 * running last means that can only ever affect this file.
 *
 * A fake address is used throughout: the point is that the notice reaches other
 * nodes and changes what they allow, not that anything is really published
 * there.
 */

// Base58, 32 bytes — the contract requires a well-formed contract id so the UI
// can build a URL from it without sanitising free text.
const NEW_ADDRESS = "68i7VVAF3rDF47TqnKCys8ewXewEacJqkVcF6wXZsE1J";

test("a migration notice moves everyone on without cutting a game short", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  // A game that is already under way before the notice goes out.
  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);
  const bobGame = await joinGame(bob.page, PEER_PORT, id);
  await expectSeated(alice.app, "bob");
  await playMove(alice.app, "e2", "e4");
  await expect.poll(() => moveList(bobGame)).toContain("e4");

  // Alice is the admin (claimed in admin.spec.ts) and announces the move.
  await alice.app.locator("#admin-button").click({ force: true });
  await alice.app.locator("#migration-address").fill(NEW_ADDRESS);
  await alice.app.locator("#migration-message").fill("moved for a stdlib fix");
  await alice.app.locator("#announce-migration").click();
  await alice.page.keyboard.press("Escape");

  try {
    // Bob's node learns of it, and is told where to go. The URL is built from
    // his own origin, because the app is served by whichever node he is on.
    const bobHome = await reopen(bob.page, PEER_PORT);
    await expect(bobHome.locator("#migration-notice")).toBeVisible();
    await expect(bobHome.locator("#migration-new-address")).toContainText(
      `/v1/contract/web/${NEW_ADDRESS}/`,
    );
    await expect(bobHome.locator("#migration-notice")).toContainText(
      "moved for a stdlib fix",
    );

    // New games are closed here.
    await expect(
      bobHome.getByRole("button", { name: "New game" }),
    ).toBeDisabled();

    // But the game already in progress is untouched: still listed, and Bob can
    // still answer. This is the guarantee the whole feature turns on.
    const bobBack = await reopen(bob.page, PEER_PORT, `?game=${id}`);
    await expect(bobBack.locator(".board")).toBeVisible();
    await playMove(bobBack, "e7", "e5", "black");
    await expect.poll(() => moveList(alice.app)).toContain("e5");
  } finally {
    // Call it off, whatever happened above — a lobby left locked would make
    // every later run of the suite fail for an unrelated reason.
    const adminFrame = await reopen(alice.page, GATEWAY_PORT);
    await adminFrame.locator("#admin-button").click({ force: true });
    await adminFrame.locator("#cancel-migration").click();
    // The cancel button only exists while a migration is announced, so its
    // disappearance is the signal that the notice is really gone.
    await expect(adminFrame.locator("#cancel-migration")).toHaveCount(0);
  }

  const reopened = await reopen(bob.page, PEER_PORT);
  await expect(reopened.locator("#migration-notice")).toHaveCount(0);
  await expect(reopened.getByRole("button", { name: "New game" })).toBeEnabled();

  await alice.context.close();
  await bob.context.close();
});
