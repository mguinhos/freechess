import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT,
  PEER_PORT,
  createGame,
  expectSeated,
  moveList,
  openApp,
  playMove,
  reopen,
} from "./helpers";

/**
 * The notification count in the header.
 *
 * A direct challenge is the one thing here that is aimed at a specific player
 * and then waits. Everything else sits in a list people browse; a challenge is
 * addressed, so without a count in the header it can go unseen indefinitely.
 *
 * The count is of challenges still waiting, not of things unread — accepting
 * one clears it by itself, with no record of what a device has seen and so
 * nothing that can drift out of step with the network. These tests hold that
 * to both ends: it appears when a challenge arrives, and goes when it is taken.
 */

test("a direct challenge raises a count in the other player's header", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  // Bob has to be visible as online before Alice can address him. Scope to the
  // players panel: his name also shows up in the ranking and on any game he
  // has, and those rows carry no Challenge button.
  const aliceHome = await reopen(alice.page, GATEWAY_PORT);
  const bobRow = aliceHome
    .locator("#players-panel .list-item", { hasText: "bob" })
    .first();
  await expect(bobRow).toBeVisible();

  // Nothing is waiting for Bob yet.
  const bobHome = await reopen(bob.page, PEER_PORT);
  await expect(bobHome.locator("#notifications-button")).toBeVisible();
  await expect(bobHome.locator("#notifications-count")).toHaveCount(0);

  await bobRow.getByRole("button", { name: "Challenge" }).click();
  const modal = aliceHome.locator(".modal");
  await expect(modal).toBeVisible();
  await modal.getByRole("button", { name: "10+0", exact: true }).click();
  // A direct challenge is confirmed with "Send challenge", not "Create game" —
  // the same modal, but it knows it is addressed to someone.
  await modal.getByRole("button", { name: "Send challenge" }).click();
  await expect(aliceHome.locator(".board")).toBeVisible();

  // It reaches Bob's node and shows up as a count, from wherever he is.
  const bobAgain = await reopen(bob.page, PEER_PORT);
  await expect(bobAgain.locator("#notifications-count")).toHaveText("1");

  // Opening it names the challenger and offers the seat.
  await bobAgain.locator("#notifications-button").click({ force: true });
  await expect(bobAgain.locator(".modal")).toContainText("alice");
  await bobAgain.locator(".modal").getByRole("button", { name: "Play" }).click();
  await expect(bobAgain.locator(".board")).toBeVisible();

  // The count must clear as soon as Bob answers — NOT when Alice gets round to
  // countersigning. Taking a seat is a two-step handshake and only the first
  // step is his, so a badge that stayed lit until the creator acted would be
  // still asking for the one thing he had just done. It read as "accepting does
  // not mark it as read", and it was reported that way.
  await expect(bobAgain.locator("#notifications-count")).toHaveCount(0);

  // Alice countersigns, and the game is real — the notification led somewhere.
  await expectSeated(aliceHome, "bob");
  await playMove(aliceHome, "e2", "e4");
  await expect.poll(() => moveList(bobAgain)).toContain("e4");

  const cleared = await reopen(bob.page, PEER_PORT);
  await expect(cleared.locator("#notifications-count")).toHaveCount(0);

  await alice.context.close();
  await bob.context.close();
});

test("an open game raises no count for anyone", async ({ browser }) => {
  // Only *addressed* challenges count. An open game is browsable by everyone,
  // so counting it would make the badge permanent noise.
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await createGame(alice.app, "10+0");

  const bobHome = await reopen(bob.page, PEER_PORT);
  await expect(bobHome.locator("#notifications-button")).toBeVisible();
  await expect(bobHome.locator("#notifications-count")).toHaveCount(0);
  await bobHome.locator("#notifications-button").click({ force: true });
  await expect(bobHome.locator("#no-notifications")).toBeVisible();

  await alice.context.close();
  await bob.context.close();
});
