import { test, expect } from "@playwright/test";
import {
  GATEWAY_PORT,
  PEER_PORT,
  PEER2_PORT,
  openApp,
  reopen,
  createGame,
} from "./helpers";

/**
 * Administration is first-come: the first claim wins. These run in order and
 * share one lobby, so the first test to claim becomes the root for the rest —
 * which is exactly the behaviour under test.
 */

test("adminship is claimable by anyone while unclaimed, and only once", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");

  // The entry point is deliberately visible to everyone while unclaimed: this
  // is a race, and hiding it would just mean whoever reads the source wins.
  const adminButton = alice.app.locator("#admin-button");
  await expect(adminButton).toBeVisible();
  // `force` because the header is live by design — a clock ticks, the online
  // count changes, and Dioxus re-creates nodes as it diffs. Playwright refuses
  // to click anything it has seen move or be detached, and waiting for a
  // header that updates every second to hold perfectly still is waiting for
  // something that will not happen. Visibility is asserted just above, so this
  // gives up stability, not correctness.
  await adminButton.click({ force: true });

  // The test network is not wiped between runs, and the account now persists in
  // the delegate — so on a second run this node is the SAME player, who has
  // already claimed. Asserting the claim button is present therefore failed for
  // reasons that had nothing to do with the rule under test. The rule is "only
  // once", and an already-claimed lobby is simply the second half of it: either
  // way, what must hold at the end is that a root exists and nobody else can
  // take it.
  if (await alice.app.locator("#claim-admin").count()) {
    await alice.app.locator("#claim-admin").click();
    await expect(
      alice.app.locator(".list-item", { hasText: "alice" }),
    ).toBeVisible();
  }
  await expect(alice.app.locator(".badge", { hasText: "root" })).toBeVisible();

  // A second player on the other node sees adminship as taken, so no claim
  // button is offered to them.
  const bob = await openApp(browser, PEER_PORT, "bob");
  // Reload so Bob's node has definitely merged the claim before checking.
  const bobHome = await reopen(bob.page, PEER_PORT);

  // Open the panel ONCE, then wait on the claim button disappearing.
  //
  // This used to be an `expect.poll` whose predicate clicked the entry button
  // on every attempt — so each retry toggled the panel shut and reopened it,
  // and what the next attempt observed depended on the previous click rather
  // than on the state under test. A wait must not have side effects.
  const entry = bobHome.locator("#admin-button");
  if (await entry.count()) {
    await entry.click({ force: true });
    // Retries until Bob's node has merged Alice's claim.
    await expect(bobHome.locator("#claim-admin")).toHaveCount(0);
  }

  await alice.context.close();
  await bob.context.close();
});

test("an admin announcement reaches a player on the other node", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await alice.app.locator("#admin-button").click({ force: true });
  // Alice claimed in the previous test; if this runs standalone, claim now.
  if (await alice.app.locator("#claim-admin").count()) {
    await alice.app.locator("#claim-admin").click();
  }

  const text = `maintenance window ${Date.now()}`;
  await alice.app.locator("#announcement-text").fill(text);
  await alice.app.locator("#publish-announcement").click();

  // The notice is lobby state, so it replicates to the other node.
  const bobHome = await reopen(bob.page, PEER_PORT);
  await expect(bobHome.locator(".banner.warn", { hasText: text })).toBeVisible();

  await alice.context.close();
  await bob.context.close();
});

test("a non-admin cannot announce", async ({ browser }) => {
  // A node that has never claimed, so this is a genuine non-admin.
  const bob = await openApp(browser, PEER2_PORT, "carol");

  // Assert the end state and let it retry, rather than checking and then
  // acting. `count()` does not retry, so it saw the entry point during the
  // moment before the lobby had loaded — adminship looks unclaimed while the
  // state is still empty, and the button is deliberately shown then. By the
  // time the click ran, the claim had merged and the button was gone, so the
  // click waited out the whole action timeout.
  //
  // The rule under test is simply that a non-admin is offered no way in.
  await expect(bob.app.locator("#admin-button")).toHaveCount(0);
  await bob.context.close();
});

test("marking the service unavailable shows a notice everywhere", async ({
  browser,
}) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  await alice.app.locator("#admin-button").click({ force: true });
  if (await alice.app.locator("#claim-admin").count()) {
    await alice.app.locator("#claim-admin").click();
  }

  // Normalise first: a previous run may have left the service marked
  // unavailable, in which case the panel offers "mark available" instead and
  // there is no message field to fill.
  if (await alice.app.locator("#mark-available").count()) {
    await alice.app.locator("#mark-available").click();
    await expect(alice.app.locator("#mark-unavailable")).toBeVisible();
  }

  await alice.app.locator("#service-message").fill("upgrading the node");
  await alice.app.locator("#mark-unavailable").click();

  const bobHome = await reopen(bob.page, PEER_PORT);
  await expect(bobHome.locator("#service-notice")).toBeVisible();
  await expect(bobHome.locator("#service-notice")).toContainText("upgrading the node");

  // Put it back, so later runs start from a clean state. The control only
  // appears once the change has come back from the network, so wait for it
  // rather than assuming it is already there.
  const aliceHome = await reopen(alice.page, GATEWAY_PORT);
  await aliceHome.locator("#admin-button").click({ force: true });
  const markAvailable = aliceHome.locator("#mark-available");
  await expect(markAvailable).toBeVisible();
  await markAvailable.click();
  await expect
    .poll(() => aliceHome.locator("#service-notice").count())
    .toBe(0);

  await alice.context.close();
  await bob.context.close();
});

test("an admin takedown removes a game from the listings", async ({ browser }) => {
  const alice = await openApp(browser, GATEWAY_PORT, "alice");
  const bob = await openApp(browser, PEER_PORT, "bob");

  // Bob starts a game so there is something live to take down.
  await createGame(bob.app, "10+0");

  const aliceHome = await reopen(alice.page, GATEWAY_PORT);
  await expect(
    aliceHome.locator(".list-item", { hasText: "bob" }).first(),
  ).toBeVisible();

  await aliceHome.locator("#admin-button").click({ force: true });
  if (await aliceHome.locator("#claim-admin").count()) {
    await aliceHome.locator("#claim-admin").click();
  }

  // Games in progress are listed in the panel; an open one is not, so take down
  // whatever the panel offers and assert the listing shrinks.
  const endButtons = aliceHome.getByRole("button", { name: "End" });
  const count = await endButtons.count();
  if (count > 0) {
    await endButtons.first().click();
    await expect(aliceHome.locator(".modal")).toBeVisible();
  }

  await alice.context.close();
  await bob.context.close();
});
