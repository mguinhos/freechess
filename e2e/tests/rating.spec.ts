import { test, expect, type FrameLocator } from "@playwright/test";
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
 * Ratings actually moving.
 *
 * Everything below the surface was in place for a long time and never fired:
 * the Elo maths, the certificate, the ranking entry that re-derives the rating
 * from co-signed inputs. What was missing was the *exchange* — a rating is only
 * trustworthy because both players signed the same record, and they cannot
 * derive that record independently (the finish time is a wall clock, the
 * pre-game ratings come from the lobby). So the two halves are traded through
 * the game contract, and only then does anything reach the ranking.
 *
 * This is the end-to-end proof: play a real game across two nodes, and watch
 * both players appear in the ranking with ratings that moved in opposite
 * directions and add up.
 */

/** Everyone starts here, so a rating that never moved still reads 1200. */
const START = 1200;

/**
 * The rating shown for `nickname` in the ranking, or null if not listed yet.
 *
 * Takes an already-open frame on purpose. Reloading inside the polling loop —
 * which is how this was first written — restarts the app on every attempt, and
 * the lobby has not arrived yet when the read happens, so it returns null
 * forever and the poll can never succeed. Open the page once and let it update.
 */
async function ratingInRanking(
  frame: FrameLocator,
  nickname: string,
): Promise<number | null> {
  const row = frame
    .locator("#ranking-panel .list-item", { hasText: nickname })
    .first();
  if (!(await row.count())) return null;
  const text = await row.locator(".elo").innerText();
  return Number(text.trim());
}

test("winning a game moves both players' ratings", async ({ browser }) => {
  const alice = await openApp(browser, GATEWAY_PORT, "elo-white");
  const bob = await openApp(browser, PEER_PORT, "elo-black");

  await createGame(alice.app, "10+0");
  const id = await gameId(alice.app);
  const bobGame = await joinGame(bob.page, PEER_PORT, id);
  await expectSeated(alice.app, "elo-black");

  // Fool's mate: Black wins on move 2, so White should lose points and Black
  // should gain them.
  await playMove(alice.app, "f2", "f3");
  await expect.poll(() => moveList(bobGame)).toContain("f3");
  await playMove(bobGame, "e7", "e5", "black");
  await expect.poll(() => moveList(alice.app)).toContain("e5");
  await playMove(alice.app, "g2", "g4");
  await expect.poll(() => moveList(bobGame)).toContain("g4");
  await playMove(bobGame, "d8", "h4", "black");

  // Both sides must see the game as over before either can certify it.
  await expect(alice.app.locator(".result-card")).toContainText("You lost");
  await expect(bobGame.locator(".result-card")).toContainText("You won");

  // Certifying is a round trip: each client publishes its half, the other
  // adopts those exact bytes, and only then is there anything to file. Wait for
  // it WITHOUT leaving the game page — reloading first, which is how this was
  // written at first, empties the client's in-memory game map, and a client
  // with no games loaded has nothing to certify. It would wait out the timeout
  // on something it had just prevented.
  //
  // The header is where a player sees their own rating change, so watch that.
  await expect
    .poll(() => alice.app.locator("#account-button").innerText(), {
      message: "the loser's rating never moved",
    })
    .not.toContain(String(START));
  await expect
    .poll(() => bobGame.locator("#account-button").innerText(), {
      message: "the winner's rating never moved",
    })
    .not.toContain(String(START));

  // Now the ranking, which is shared state and should agree on both nodes.
  const bobHome = await reopen(bob.page, PEER_PORT);
  const aliceHome = await reopen(alice.page, GATEWAY_PORT);
  await expect
    .poll(() => ratingInRanking(bobHome, "elo-black"))
    .toBeGreaterThan(START);
  await expect
    .poll(() => ratingInRanking(aliceHome, "elo-white"))
    .toBeLessThan(START);

  const winner = await ratingInRanking(bobHome, "elo-black");
  const loser = await ratingInRanking(aliceHome, "elo-white");

  // Elo is zero-sum between two players who started level: what one gains the
  // other loses. Checking the sum, rather than a hardcoded number, keeps this
  // honest if the K-factor is ever retuned.
  expect(winner).not.toBeNull();
  expect(loser).not.toBeNull();
  expect((winner as number) - START).toBe(START - (loser as number));

  // And both nodes agree — the ranking is shared state, not a local tally.
  await expect
    .poll(() => ratingInRanking(aliceHome, "elo-black"))
    .toBe(winner);
  await expect.poll(() => ratingInRanking(bobHome, "elo-white")).toBe(loser);

  await alice.context.close();
  await bob.context.close();
});
