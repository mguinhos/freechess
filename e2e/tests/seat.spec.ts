import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT,
  PEER_PORT,
  PEER2_PORT,
  createGame,
  gameId,
  moveList,
  openApp,
  playMove,
  reopen,
} from "./helpers";

/**
 * The seat handshake, across real nodes.
 *
 * Joining a game publishes an *offer*; the seat is only filled when the
 * creator countersigns it, which their client does on its own. That is what
 * closes the hijack the unit tests cover
 * (`a_backdated_join_cannot_hijack_a_game_in_progress`): the seat follows a
 * signature from a key fixed in the contract parameters, not a self-asserted
 * join time.
 *
 * The unit tests prove the rule. These prove the *handshake actually completes*
 * over the network, unattended — a rule nobody can satisfy would look identical
 * to a game that simply never starts.
 */

test("the creator's client seats a challenger without anyone clicking twice", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);

  // Bob offers. This is the only click he makes.
  //
  // Addressed by game id, not by the creator's name in the lobby: nicknames are
  // resolved live from presence, so every game a player ever created is
  // relabelled the moment they rename. Matching on the name then picks an
  // arbitrary one of them, which is what made the first draft of this test fail.
  const bobHome = await reopen(bob.page, PEER_PORT, `?game=${id}`);
  await expect(bobHome.locator(".board")).toBeVisible();
  await bobHome.getByRole("button", { name: "Take the open seat" }).click();

  // Alice does nothing at all. Her client has to notice the offer and
  // countersign it on its own, and until it does the seat label reads
  // "waiting…" and no move is legal for either side.
  //
  // This wait IS the assertion: Bob's name appearing on Alice's board means the
  // countersignature landed. Moving without waiting for it silently clicks into
  // a board that is not yet playable, which is how the first draft of this test
  // failed while the handshake was in fact working.
  await expect(
    alice.app.locator(".seat").getByText("bob", { exact: true }),
  ).toBeVisible();

  await playMove(alice.app, "e2", "e4");
  await expect
    .poll(() => moveList(alice.app), {
      message: "the seat was never filled — Alice's client never countersigned",
    })
    .toContain("e4");

  // And the seat is really Bob's: he can answer.
  await expect.poll(() => moveList(bobHome)).toContain("e4");
  await playMove(bobHome, "e7", "e5", "black");
  await expect.poll(() => moveList(alice.app)).toContain("e5");

  // Both sides agree on who is playing.
  await expect(alice.app.getByText("You are playing this game")).toBeVisible();
  await expect(bobHome.getByText("You are playing this game")).toBeVisible();

  await alice.context.close();
  await bob.context.close();
});

test("a latecomer on a third node cannot take a seat that is already filled", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice2");
  const bob = await openApp(browser, PEER_PORT, "bob2");

  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);

  const bobHome = await reopen(bob.page, PEER_PORT, `?game=${id}`);
  await expect(bobHome.locator(".board")).toBeVisible();
  await bobHome.getByRole("button", { name: "Take the open seat" }).click();

  // Wait for the countersignature before moving — see the first test.
  await expect(
    alice.app.locator(".seat").getByText("bob2", { exact: true }),
  ).toBeVisible();

  // Play a move, so the game is demonstrably under way before the latecomer
  // arrives. This is the state the old race rule destroyed.
  await playMove(alice.app, "d2", "d4");
  await expect.poll(() => moveList(bobHome)).toContain("d4");

  // Carol arrives from a third node — a third node and not a second tab,
  // because identity is per node: the delegate holds the account.
  const carol = await openApp(browser, PEER2_PORT, "carol2");
  const carolGame = await reopen(carol.page, PEER2_PORT, `?game=${id}`);
  await expect(carolGame.locator(".board")).toBeVisible();

  // The seat is filled, so she is offered no way in and is told she is a
  // spectator.
  await expect(carolGame.getByText("You are spectating")).toBeVisible();
  await expect(
    carolGame.getByRole("button", { name: "Take the open seat" }),
  ).toHaveCount(0);

  // The game is untouched: the move list still holds Bob's game, and Bob is
  // still a player who can move.
  await playMove(bobHome, "d7", "d5", "black");
  await expect.poll(() => moveList(alice.app)).toContain("d5");
  await expect.poll(() => moveList(carolGame)).toContain("d5");
  await expect(bobHome.getByText("You are playing this game")).toBeVisible();

  await alice.context.close();
  await bob.context.close();
  await carol.context.close();
});
