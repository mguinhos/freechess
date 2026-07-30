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
 * The published webapp's contract id, written by `cargo make publish-webapp`.
 * Read from disk rather than hardcoded so the tests follow a republish.
 */
export function contractId(): string {
  const path = join(__dirname, "../../target/webapp/contract-id");
  try {
    return readFileSync(path, "utf8").trim();
  } catch {
    throw new Error(
      `could not read ${path} — publish the app first with 'cargo make publish'`,
    );
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
  await expect(frame.locator(".topbar")).toBeVisible();
  // Wait for the node connection before doing anything that needs the network.
  await expect(frame.locator(".conn .dot.online")).toBeVisible();

  if (nickname) await setNickname(frame, nickname);
  return { context, page, app: frame, errors };
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
 * Squares carry no test id, so they are addressed by index. The board is laid
 * out rank 8 → rank 1, file a → h when oriented for White.
 */
export async function clickSquare(
  frame: FrameLocator,
  square: string,
  orientation: "white" | "black" = "white",
) {
  const file = square.charCodeAt(0) - "a".charCodeAt(0);
  const rank = parseInt(square[1], 10) - 1;
  const index =
    orientation === "white" ? (7 - rank) * 8 + file : rank * 8 + (7 - file);
  await frame.locator(".board > div").nth(index).click();
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
