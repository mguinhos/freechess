// Play FreeChess from the command line, driving the real UI in a browser.
//
// This is how the assistant takes a turn: same app, same signatures, same
// contracts a human player uses — nothing bypasses the UI, so anything that
// works here works for a person too.
//
// The account is pinned via localStorage so every invocation is the *same*
// player. Without that, each run would mint a fresh key and the contract would
// (correctly) refuse the moves as coming from a stranger.
//
//   node e2e/play.mjs create  [--time 10+0] [--color white]
//   node e2e/play.mjs show    --game <id>
//   node e2e/play.mjs move    --game <id> --uci e2e4
//   node e2e/play.mjs lobby
//
// --port selects the node (default 7513, the public one).

import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const args = process.argv.slice(2);
const cmd = args[0];

function flag(name, fallback = undefined) {
  const i = args.indexOf(`--${name}`);
  return i === -1 ? fallback : args[i + 1];
}

const PORT = flag("port", "7513");
// Read the address from the committed artifact rather than hardcoding it. A
// hardcoded id goes stale the moment the app is re-keyed, and this file sat
// pointing at a long-dead contract for exactly that reason.
const CONTRACT = readFileSync(
  new URL("../published-contract/contract-id.txt", import.meta.url),
  "utf8",
).trim();
const APP = `http://127.0.0.1:${PORT}/v1/contract/web/${CONTRACT}/`;

// A fixed account, so this player keeps their identity across invocations.
// Generated once and hardcoded on purpose: it is a throwaway test identity.
const ACCOUNT = process.env.FREECHESS_ACCOUNT ?? "";
const NICKNAME = process.env.FREECHESS_NICK ?? "claude";

async function open(query = "") {
  const browser = await chromium.launch();
  const context = await browser.newContext();
  // Seed the account before any script runs, so the app adopts it on load.
  if (ACCOUNT) {
    await context.addInitScript(
      ([key, nick]) => {
        localStorage.setItem("freechess:account:v1", key);
        localStorage.setItem("freechess:nickname:v1", nick);
      },
      [ACCOUNT, NICKNAME],
    );
  }
  const page = await context.newPage();
  page.on("pageerror", (e) => console.error("page error:", String(e)));
  await page.goto(APP + query, { waitUntil: "domcontentloaded" });
  const app = page.frameLocator("#app");
  await app.locator(".topbar").waitFor({ timeout: 60000 });
  await app.locator(".conn .dot.online").waitFor({ timeout: 60000 });
  await waitForAccount(app);
  return { browser, page, app };
}

/**
 * Wait until the account has come back from the delegate.
 *
 * This is not optional. Identity lives in the node's delegate, and fetching it
 * is a round trip: for the first seconds after load the app is running on a
 * freshly generated session key instead. Acting in that window makes every
 * invocation a *different* player — moves signed by a stranger, which the
 * contract correctly refuses — and it is why an earlier run created a game as
 * `player-HAebfp` and came back as `player-UW2AbB`.
 *
 * There is no "settled" flag in the DOM, so watch the label the app shows and
 * wait for it to CHANGE — the change *is* the adoption. Waiting for it to hold
 * still does not work, and was the first thing I tried: the label sits on the
 * session key perfectly still until the answer arrives, so two equal readings
 * are satisfied instantly and prove nothing.
 *
 * On a node whose delegate has never held an account there is nothing to adopt
 * and nothing to change, so fall back to the timeout — from then on the seed is
 * stored and later runs adopt it.
 */
async function waitForAccount(app) {
  const initial = (await app.locator("#account-button").innerText()).trim();
  for (let attempt = 0; attempt < 30; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 1000));
    const id = (await app.locator("#account-button").innerText()).trim();
    if (id !== initial) return id;
  }
  console.error(
    `note: the account never changed from ${initial} — assuming this node's ` +
      `delegate had nothing stored, and this session's key is now it`,
  );
  return initial;
}

/** Click a square by algebraic name, given the board orientation. */
async function clickSquare(app, square, orientation) {
  const file = square.charCodeAt(0) - 97;
  const rank = parseInt(square[1], 10) - 1;
  const index =
    orientation === "white" ? (7 - rank) * 8 + file : rank * 8 + (7 - file);
  await app.locator(".board > div").nth(index).click();
}

/**
 * Which way round the board is drawn for us.
 *
 * Reading this matters: with the board flipped, clicking as though it were
 * White's view lands on the mirrored square, which is either an illegal move
 * or — worse — a legal one nobody intended.
 *
 * The seats are NOT viewer-relative: the app renders Black on top and White
 * below, always, and flips only the board. So the orientation is the colour of
 * whichever seat is ours, found by name. (Two players sharing a nickname would
 * defeat that; there are two seats and it is our own node, so it holds here.)
 */
async function myOrientation(app) {
  const seats = app.locator(".seat");
  const count = await seats.count();
  for (let i = 0; i < count; i++) {
    const text = await seats.nth(i).innerText();
    // Case-insensitively: the nickname shown is whatever was typed into the
    // account modal, which need not match the constant's casing.
    if (!text.toLowerCase().includes(NICKNAME.toLowerCase())) continue;
    const cls =
      (await seats.nth(i).locator(".piece").first().getAttribute("class")) ?? "";
    return cls.includes("black") ? "black" : "white";
  }
  // Spectating, or a name we do not recognise: White's view is what the app
  // falls back to as well.
  return "white";
}

/** Print the board, move list and clocks. */
async function report(app) {
  const moves = await app.locator(".moves .mv").allInnerTexts();
  console.log("moves:", moves.length ? moves.join(" ") : "(none yet)");

  const clocks = await app.locator(".clock").allInnerTexts();
  if (clocks.length) console.log("clocks:", clocks.join("  |  "));

  if (process.env.FREECHESS_DEBUG) {
    const seats = app.locator(".seat");
    console.log("seats:", await seats.count(), "orientation:", await myOrientation(app));
    for (let i = 0; i < (await seats.count()); i++) {
      const p = seats.nth(i).locator(".piece").first();
      console.log(
        `  seat[${i}] pieceClass=${(await p.count()) ? await p.getAttribute("class") : "-"} text=${(await seats.nth(i).innerText()).replace(/\n/g, " | ")}`,
      );
    }
  }

  const result = app.locator(".result-card .headline");
  if (await result.count()) console.log("RESULT:", await result.innerText());

  // Always print from White's point of view, whichever way the app drew it.
  // Black's layout is the exact reverse of White's, so one reverse normalises
  // it — without this the rank and file labels are mirrored and the position
  // reads as a completely different one.
  let squares = await app.locator(".board > div").allInnerTexts();
  if ((await myOrientation(app)) === "black") squares = squares.slice().reverse();
  if (squares.length === 64) {
    console.log();
    for (let r = 0; r < 8; r++) {
      const row = squares
        .slice(r * 8, r * 8 + 8)
        .map((s) => (s.trim() === "" ? "." : s.trim()))
        .join(" ");
      console.log(`${8 - r}  ${row}`);
    }
    console.log("   a b c d e f g h");
  }
}

async function main() {
  if (cmd === "create") {
    const time = flag("time", "10+0");
    const color = flag("color", "white");
    const { browser, app } = await open();
    await app.getByRole("button", { name: "New game" }).click();
    await app.getByRole("button", { name: time, exact: true }).click();
    await app
      .getByRole("button", { name: color === "white" ? "White" : "Black", exact: true })
      .click();
    await app.getByRole("button", { name: "Create game" }).click();
    await app.locator(".board").waitFor({ timeout: 60000 });
    const link = await app.locator("#share-link").innerText();
    console.log("created:", link.trim());
    await report(app);
    await browser.close();
    return;
  }

  if (cmd === "nick") {
    // The account lives in the node's delegate, not in localStorage — the
    // gateway sandboxes the app without allow-same-origin, so localStorage
    // throws. So the nickname has to be set through the UI, once, and it then
    // sticks for this node.
    const name = flag("name", NICKNAME);
    const { browser, app } = await open();
    await app.locator("#account-button").click();
    const modal = app.locator(".modal");
    await modal.waitFor({ timeout: 60000 });
    await modal.locator("input").first().fill(name);
    await modal.getByRole("button", { name: "Save" }).click();
    await modal.waitFor({ state: "hidden", timeout: 60000 });
    console.log("nickname set to:", name);
    await browser.close();
    return;
  }

  if (cmd === "export") {
    // The account seed lives in the node's delegate; this is the only way to
    // get it out. Whoever holds the string IS this player, so treat it as a
    // secret — never commit it, and never publish it anywhere the repo reaches.
    const { browser, app } = await open();
    await app.locator("#account-button").click();
    const modal = app.locator(".modal");
    await modal.waitFor({ timeout: 60000 });
    await modal.getByRole("button", { name: "Reveal key" }).click();
    const key = (await modal.locator("code.key").innerText()).trim();
    const id = (await modal.locator(".mono").first().innerText()).trim();
    console.log("player_id  ", id);
    console.log("account_key", key);
    await browser.close();
    return;
  }

  if (cmd === "whoami") {
    const { browser, page, app } = await open();
    page.on("console", (m) => {
      if (m.type() === "error") console.error("console:", m.text());
    });
    console.log("topbar:", (await app.locator("#account-button").innerText()).trim());
    const msg = app.locator("#app-message");
    if (await msg.count()) console.log("app message:", (await msg.innerText()).trim());
    await browser.close();
    return;
  }

  if (cmd === "lobby") {
    const { browser, app } = await open();
    const items = await app.locator(".list-item").allInnerTexts();
    console.log(items.length ? items.join("\n") : "(lobby empty)");
    const live = await app.locator(".game-card").count();
    console.log(`live games: ${live}`);
    await browser.close();
    return;
  }

  const game = flag("game");
  if (!game) throw new Error("--game <id> is required");
  const { browser, app } = await open(`?game=${game}`);
  await app.locator(".board").waitFor({ timeout: 60000 });

  if (cmd === "show") {
    await report(app);
    await browser.close();
    return;
  }

  if (cmd === "move") {
    const uci = flag("uci");
    if (!uci) throw new Error("--uci <e2e4> is required");
    // Orientation follows the side we are playing; the app orients the board
    // for the viewer, so read it from the app rather than assuming.
    const spectating = await app.getByText("You are spectating").count();
    if (spectating) throw new Error("this account is not a player in that game");

    const orientation = await myOrientation(app);
    const before = (await app.locator(".moves .mv").allInnerTexts()).length;

    await clickSquare(app, uci.slice(0, 2), orientation);
    await clickSquare(app, uci.slice(2, 4), orientation);

    // Wait for the move to register rather than assuming it did.
    for (let i = 0; i < 60; i++) {
      const now = (await app.locator(".moves .mv").allInnerTexts()).length;
      if (now > before) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    await report(app);
    await browser.close();
    return;
  }

  throw new Error(`unknown command: ${cmd}`);
}

main().catch((e) => {
  console.error(String(e));
  process.exit(1);
});
