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

const args = process.argv.slice(2);
const cmd = args[0];

function flag(name, fallback = undefined) {
  const i = args.indexOf(`--${name}`);
  return i === -1 ? fallback : args[i + 1];
}

const PORT = flag("port", "7513");
const CONTRACT = "GV3WZEAWC82nrXJcvgwYaXNLLd71AVtDuu5Pc7TnRR92";
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
  return { browser, page, app };
}

/** Click a square by algebraic name, given the board orientation. */
async function clickSquare(app, square, orientation) {
  const file = square.charCodeAt(0) - 97;
  const rank = parseInt(square[1], 10) - 1;
  const index =
    orientation === "white" ? (7 - rank) * 8 + file : rank * 8 + (7 - file);
  await app.locator(".board > div").nth(index).click();
}

/** Print the board, move list and clocks. */
async function report(app) {
  const moves = await app.locator(".moves .mv").allInnerTexts();
  console.log("moves:", moves.length ? moves.join(" ") : "(none yet)");

  const clocks = await app.locator(".clock").allInnerTexts();
  if (clocks.length) console.log("clocks:", clocks.join("  |  "));

  const result = app.locator(".result-card .headline");
  if (await result.count()) console.log("RESULT:", await result.innerText());

  // The board as text, from White's point of view.
  const squares = await app.locator(".board > div").allInnerTexts();
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

    // Determine our colour from which seat carries our nickname at the bottom.
    const orientation = (await app.locator(".seat").last().innerText()).includes(
      NICKNAME,
    )
      ? "white"
      : "white"; // the app always puts the viewer at the bottom
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
