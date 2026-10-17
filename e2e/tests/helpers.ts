import {
  expect,
  Page,
  BrowserContext,
  Browser,
  FrameLocator,
} from "@playwright/test";
import { readFileSync } from "fs";
import { join } from "path";

/**
 * HTTP ports of the two test nodes. These MUST match scripts/nodes.sh.
 *
 * Deliberately not 7509/7510: a developer very likely has their own node on
 * 7509, and pointing the suite there both fails (that node has never seen our
 * contract) and interferes with whatever they are running.
 */
export const GATEWAY_PORT = 7511;
export const PEER_PORT = 7512;

/**
 * A third node.
 *
 * Identity is per-NODE, not per-tab: the account lives in the delegate, which
 * belongs to the node. So two browser contexts pointed at one node are the same
 * player, and a test needing three distinct people needs three nodes.
 */
export const PEER2_PORT = 7514;

/**
 * The published webapp's contract id.
 *
 * Read from the committed `published-contract/`, which is the address the app
 * is actually published to — not from a build output. A build output would go
 * stale or vanish on `cargo clean`, and worse, could disagree with where the
 * publish landed.
 */
export function contractId(): string {
  if (process.env.FREECHESS_CONTRACT_ID) return process.env.FREECHESS_CONTRACT_ID;
  const path = join(__dirname, "../../published-contract/contract-id.txt");
  try {
    return readFileSync(path, "utf8").trim();
  } catch {
    throw new Error(`could not read ${path} — is published-contract/ committed?`);
  }
}

export function appUrl(port: number, query = ""): string {
  return `http://127.0.0.1:${port}/v1/contract/web/${contractId()}/${query}`;
}

/**
 * The app runs inside a sandboxed iframe.
 *
 * The gateway does not serve the webapp directly: it serves a shell page whose
 * `<iframe id="app" sandbox="allow-scripts ...">` loads the real bundle. That
 * sandbox is the whole point — it is what stops a webapp from reading the
 * node's cookies or reaching other origins. Every locator therefore has to go
 * through the frame, not the top-level page.
 */
export function app(page: Page): FrameLocator {
  return page.frameLocator("#app");
}

/** A loaded app session: the page, its frame, and any console errors. */
export interface Session {
  context: BrowserContext;
  page: Page;
  app: FrameLocator;
  errors: string[];
}

/**
 * Open the app on a given node as a fresh player.
 *
 * A new browser context means new localStorage, and the app generates an
 * account on first load — so each context is a distinct player.
 */
export async function openApp(
  browser: Browser,
  port: number,
  nickname?: string,
): Promise<Session> {
  const context = await browser.newContext();
  const page = await context.newPage();

  // Collect console errors: a WASM panic or a CSP violation shows up here and
  // nowhere else, and would otherwise look like a mysterious timeout.
  const errors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(msg.text());
  });
  page.on("pageerror", (err) => errors.push(String(err)));

  await page.goto(appUrl(port), { waitUntil: "domcontentloaded" });
  const frame = app(page);
  if (nickname) {
    await page.addLocatorHandler(frame.locator("#nickname-prompt"), async () => {
      await frame.locator("#nickname-later").click();
    });
  }
  await expect(frame.locator(".topbar")).toBeVisible();
  // Wait for the node connection before doing anything that needs the network.
  await expect(frame.locator(".conn .dot.online")).toBeVisible();
  await waitForAccount(frame);

  if (nickname) await setNickname(frame, nickname);
  return { context, page, app: frame, errors };
}

/**
 * Wait until identity has come back from the delegate.
 *
 * Not optional, and not cosmetic. For the first seconds after a load the app is
 * running on a session key it generated itself; anything signed in that window
 * belongs to a player who ceases to exist as soon as the stored account
 * arrives. That is what made adminship flaky — a claim made too early was made
 * by a stranger, and the next page load was not an admin.
 */
export async function waitForAccount(frame: FrameLocator) {
  await expect(
    frame.locator('#account-button[data-settled="true"]'),
  ).toBeVisible();
}

/** Reload the app in place, returning the fresh frame. */
export async function reopen(
  page: Page,
  port: number,
  query = "",
): Promise<FrameLocator> {
  await page.goto(appUrl(port, query), { waitUntil: "domcontentloaded" });
  const frame = app(page);
  await expect(frame.locator(".topbar")).toBeVisible();
  await expect(frame.locator(".conn .dot.online")).toBeVisible();
  await waitForAccount(frame);
  return frame;
}

export async function setNickname(frame: FrameLocator, nickname: string) {
  // Addressed by id, not position: the top bar has more than one ghost button
  // (the admin entry point sits beside the account one).
  await frame.locator("#account-button").click();
  const modal = frame.locator(".modal");
  await expect(modal).toBeVisible();
  await modal.locator("input").first().fill(nickname);
  await modal.getByRole("button", { name: "Save" }).click();
  await expect(modal).toBeHidden();
}

/**
 * Take the open seat in a specific game, addressed by id.
 *
 * Never locate a game by its creator's name. The lobby shows the nickname
 * signed into the setup, and the test network is not wiped between runs, so
 * several open games carry the same name; `.first()` then picks an arbitrary
 * one. That is how the checkmate test came to join a stale game and wait
 * forever for a move its real opponent never made.
 */
export async function joinGame(
  page: Page,
  port: number,
  id: string,
): Promise<FrameLocator> {
  const frame = await reopen(page, port, `?game=${id}`);
  await expect(frame.locator(".board")).toBeVisible();
  await frame.getByRole("button", { name: "Take the open seat" }).click();
  return frame;
}

/**
 * Wait until the creator's client has countersigned the challenger, which is
 * what fills the seat and makes any move legal.
 *
 * Joining only publishes an *offer*; clicking a square before the seat is
 * filled does nothing at all, and the test then hangs on a move that was never
 * accepted rather than on the thing it meant to check.
 */
export async function expectSeated(frame: FrameLocator, nickname: string) {
  await expect(
    frame.locator(".seat").getByText(nickname, { exact: true }),
  ).toBeVisible();
}

/** Create a game and land on its board. */
export async function createGame(frame: FrameLocator, timeControl = "10+0") {
  await frame.getByRole("button", { name: "New game" }).click();
  const modal = frame.locator(".modal");
  await expect(modal).toBeVisible();
  await modal.getByRole("button", { name: timeControl, exact: true }).click();
  await modal.getByRole("button", { name: "Create game" }).click();
  await expect(frame.locator(".board")).toBeVisible();
}

/**
 * Play a move by clicking the origin square then the destination.
 *
 * Algebraic square names stay stable when either player flips the board.
 */
export async function clickSquare(
  frame: FrameLocator,
  square: string,
  orientation: "white" | "black" = "white",
) {
  await frame.locator(`.board > .sq[data-square="${square}"]`).click();
}

export async function playMove(
  frame: FrameLocator,
  from: string,
  to: string,
  orientation: "white" | "black" = "white",
) {
  await clickSquare(frame, from, orientation);
  await clickSquare(frame, to, orientation);
}

/**
 * The current game's id, read from the share link the game panel renders.
 *
 * Deliberately NOT from `page.url()`: the gateway runs the app in a sandboxed
 * iframe without `allow-same-origin`, so the app cannot rewrite the address
 * bar, and the top-level URL stays on the bare app path. The share link is the
 * app's own answer to "what URL opens this game", so reading it also checks
 * that the shareable link is actually produced.
 */
export async function gameId(frame: FrameLocator): Promise<string> {
  const link = frame.locator("#share-link");
  await expect(link).toBeVisible();
  const text = (await link.innerText()).trim();
  const id = new URL(text, "http://placeholder").searchParams.get("game");
  if (!id) throw new Error(`share link has no game id: ${text}`);
  return id;
}

/** The SAN move list currently rendered. */
export async function moveList(frame: FrameLocator): Promise<string[]> {
  return frame.locator(".moves .mv").allInnerTexts();
}

/** Assert nothing blew up in the console. */
export function expectCleanConsole(errors: string[]) {
  // Ignore benign network noise from a peer that is still connecting.
  const real = errors.filter(
    (e) => !/favicon|net::ERR_|WebSocket is closed/i.test(e),
  );
  expect(real, `console errors:\n${real.join("\n")}`).toEqual([]);
}
