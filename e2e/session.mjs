// A long-lived FreeChess session, driven by a command file.
//
// Why not one browser per move: the gateway sandboxes the app without
// allow-same-origin, so localStorage throws and the account only lives in
// memory. Every fresh browser is therefore a *different player*, and the
// contract correctly rejects its moves. Keeping one browser open for the whole
// game keeps one identity — which is also exactly how a human plays.
//
//   node e2e/session.mjs --port 7513 --nick claude --cmdfile /tmp/fc-cmds
//
// Then append commands, one per line:
//   create 10+0 white
//   join <gameid>
//   move e2e4
//   show
//   quit

import { chromium } from "@playwright/test";
import { existsSync, readFileSync, writeFileSync } from "fs";

const args = process.argv.slice(2);
const flag = (n, d) => {
  const i = args.indexOf(`--${n}`);
  return i === -1 ? d : args[i + 1];
};

const PORT = flag("port", "7513");
const NICK = flag("nick", "claude");
const CMDFILE = flag("cmdfile", "/tmp/freechess-cmds");
// Read from the file the publish step writes, rather than a constant edited by
// hand on every release — that only ever showed up as a dirty working tree.
const CONTRACT = readFileSync(
  new URL("../target/webapp/contract-id", import.meta.url),
  "utf8",
).trim();
const APP = `http://127.0.0.1:${PORT}/v1/contract/web/${CONTRACT}/`;

let page, app, myColor = "white";

const log = (...m) => console.log(`[${new Date().toISOString().slice(11, 19)}]`, ...m);

async function board() {
  const squares = await app.locator(".board > div").allInnerTexts();
  if (squares.length !== 64) return;
  const lines = [];
  for (let r = 0; r < 8; r++) {
    lines.push(
      `${myColor === "white" ? 8 - r : r + 1}  ` +
        squares
          .slice(r * 8, r * 8 + 8)
          .map((s) => (s.trim() === "" ? "." : s.trim()))
          .join(" "),
    );
  }
  const files = myColor === "white" ? "a b c d e f g h" : "h g f e d c b a";
  log("\n" + lines.join("\n") + `\n   ${files}`);
}

async function status() {
  const moves = await app.locator(".moves .mv").allInnerTexts();
  log("moves:", moves.length ? moves.join(" ") : "(none)");
  const clocks = await app.locator(".clock").allInnerTexts();
  if (clocks.length) log("clocks:", clocks.join(" | "));
  const res = app.locator(".result-card .headline");
  if (await res.count()) log("RESULT:", await res.innerText());
  const msg = app.locator("#app-message");
  if (await msg.count()) log("app says:", await msg.innerText());
  await board();
}

async function clickSquare(sq) {
  const file = sq.charCodeAt(0) - 97;
  const rank = parseInt(sq[1], 10) - 1;
  const index =
    myColor === "white" ? (7 - rank) * 8 + file : rank * 8 + (7 - file);
  await app.locator(".board > div").nth(index).click();
}

async function run(line) {
  const [cmd, a, b] = line.trim().split(/\s+/);
  if (!cmd) return;
  log(">>>", line.trim());

  if (cmd === "create") {
    await app.getByRole("button", { name: "New game" }).click();
    await app.getByRole("button", { name: a || "10+0", exact: true }).click();
    myColor = (b || "white").toLowerCase();
    await app
      .getByRole("button", { name: myColor === "white" ? "White" : "Black", exact: true })
      .click();
    await app.getByRole("button", { name: "Create game" }).click();
    await app.locator(".board").waitFor({ timeout: 90000 });
    log("share link:", (await app.locator("#share-link").innerText()).trim());
    await status();
    return;
  }

  if (cmd === "join") {
    await page.goto(`${APP}?game=${a}`, { waitUntil: "domcontentloaded" });
    app = page.frameLocator("#app");
    await app.locator(".board").waitFor({ timeout: 90000 });
    const take = app.getByRole("button", { name: "Take the open seat" });
    if (await take.count()) {
      await take.click();
      log("took the open seat");
    }
    // We joined, so we are whichever colour the creator left.
    myColor = "black";
    await status();
    return;
  }

  if (cmd === "move") {
    const before = (await app.locator(".moves .mv").allInnerTexts()).length;
    await clickSquare(a.slice(0, 2));
    await clickSquare(a.slice(2, 4));
    for (let i = 0; i < 120; i++) {
      const now = (await app.locator(".moves .mv").allInnerTexts()).length;
      if (now > before) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    await status();
    return;
  }

  if (cmd === "show") return status();
  if (cmd === "lobby") {
    const items = await app.locator(".list-item").allInnerTexts();
    log(items.length ? items.join(" || ") : "(lobby empty)");
    return;
  }
  if (cmd === "quit") process.exit(0);
  log("unknown command:", cmd);
}

async function main() {
  const browser = await chromium.launch();
  const context = await browser.newContext();
  page = await context.newPage();
  page.on("pageerror", (e) => log("page error:", String(e)));
  await page.goto(APP, { waitUntil: "domcontentloaded" });
  app = page.frameLocator("#app");
  await app.locator(".topbar").waitFor({ timeout: 90000 });
  await app.locator(".conn .dot.online").waitFor({ timeout: 90000 });

  // Set the nickname so the other player can recognise us.
  await app.locator("#account-button").click();
  const modal = app.locator(".modal");
  await modal.locator("input").first().fill(NICK);
  await modal.getByRole("button", { name: "Save" }).click();
  const id = await app.locator("#account-button").innerText();
  log("session ready as", id.trim());

  if (!existsSync(CMDFILE)) writeFileSync(CMDFILE, "");
  let consumed = 0;
  // Poll the command file. Simple and robust: no IPC to get wedged, and the
  // file doubles as a transcript of what was asked.
  for (;;) {
    const lines = readFileSync(CMDFILE, "utf8").split("\n").filter((l) => l.trim());
    while (consumed < lines.length) {
      const line = lines[consumed++];
      try {
        await run(line);
      } catch (e) {
        log("command failed:", String(e).split("\n")[0]);
      }
    }
    await new Promise((r) => setTimeout(r, 1500));
  }
}

main().catch((e) => {
  console.error(String(e));
  process.exit(1);
});
