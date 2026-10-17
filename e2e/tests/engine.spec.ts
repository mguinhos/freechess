import { test, expect, FrameLocator, Page } from "@playwright/test";
import { readFileSync } from "fs";
import { join } from "path";
import { app, appUrl, GATEWAY_PORT, playMove } from "./helpers";

async function openComputer(page: Page) {
  await page.addInitScript(() => {
    const engine = {
      created: 0, active: 0, terminated: 0,
      lines: [] as string[], assets: [] as string[], blobs: new Set<string>(),
      commands: [] as { worker: number; command: string }[],
    };
    (window as any).freechessEngine = engine;
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      private alive = true;
      private workerId: number;
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options);
        engine.created++;
        this.workerId = engine.created;
        engine.active++;
        this.addEventListener("message", event => {
          if (typeof event.data === "string") engine.lines.push(event.data);
        });
      }
      postMessage(message: any, options?: Transferable[] | StructuredSerializeOptions) {
        if (typeof message === "string") engine.commands.push({ worker: this.workerId, command: message });
        super.postMessage(message, options as any);
      }
      terminate() {
        if (this.alive) {
          this.alive = false;
          engine.active--;
          engine.terminated++;
        }
        super.terminate();
      }
    };
    const nativeFetch = window.fetch;
    window.fetch = function (input, init) {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href).href;
      if (/stockfish/i.test(url)) engine.assets.push(url);
      return nativeFetch.call(this, input, init);
    };
    const create = URL.createObjectURL;
    const revoke = URL.revokeObjectURL;
    URL.createObjectURL = function (blob) {
      const url = create.call(this, blob);
      engine.blobs.add(url);
      return url;
    };
    URL.revokeObjectURL = function (url) {
      engine.blobs.delete(url);
      revoke.call(this, url);
    };
  });
  await page.goto(appUrl(GATEWAY_PORT));
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  await expect(frame.locator(".board > .sq")).toHaveCount(64);
  await expect(frame.locator(".seat .name").first()).toHaveText("Stockfish 19 Lite");
  return frame;
}

async function observed(frame: FrameLocator) {
  return frame.locator("body").evaluate(() => {
    const state = (window as any).freechessEngine;
    return {
      created: state.created as number, active: state.active as number,
      terminated: state.terminated as number, blobs: state.blobs.size as number,
      lines: state.lines as string[], assets: state.assets as string[],
      commands: state.commands as { worker: number; command: string }[],
    };
  });
}

async function idle(frame: FrameLocator) {
  await expect.poll(async () => {
    const state = await observed(frame);
    return { active: state.active, blobs: state.blobs };
  }, { timeout: 20_000 }).toEqual({ active: 0, blobs: 0 });
}

test("real Stockfish replies legally, stays responsive and finds a forced mate", async ({ page }) => {
  const frame = await openComputer(page);
  const href = await frame.locator("#stockfish-credits").getAttribute("href");
  expect(href).toBeTruthy();
  const creditsUrl = new URL(href!, page.url());
  const credits = await page.request.get(creditsUrl.href);
  expect(credits.ok()).toBe(true);
  const html = await credits.text();
  for (const filename of ["Copying.txt", "SOURCE.md", "SHA256SUMS", "AUTHORS", "stockfish-19.0.1-source.tar.xz"]) {
    expect(html).toContain(`href="${filename}"`);
    const resource = await page.request.get(new URL(filename, creditsUrl).href);
    expect(resource.ok(), filename).toBe(true);
    if (filename.endsWith(".tar.xz")) {
      expect([...(await resource.body()).subarray(0, 6)]).toEqual([253, 55, 122, 88, 90, 0]);
    }
  }
  const blackPieces = await frame.locator(".board > .sq").evaluateAll(squares => Object.fromEntries(
    squares.map(square => [square.getAttribute("data-square"), square.getAttribute("data-piece")]),
  ));
  await playMove(frame, "e2", "e4");
  await expect(frame.locator("#engine-status")).toHaveAttribute("data-status", "thinking");
  const sound = frame.locator("#sound-button");
  const enabled = await sound.getAttribute("aria-pressed");
  await sound.click();
  await expect(sound).toHaveAttribute("aria-pressed", enabled === "true" ? "false" : "true");
  await sound.click();
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await expect(frame.locator("#engine-status")).toHaveAttribute("data-status", "ready");
  const state = await observed(frame);
  expect(state.lines.some(line => /^id name Stockfish 19/i.test(line))).toBe(true);
  const best = state.lines.find(line => /^bestmove /.test(line))?.split(/\s+/)[1];
  expect(best).toMatch(/^[a-h][1-8][a-h][1-8][qrbn]?$/);
  expect(blackPieces[best!.slice(0, 2)]).toMatch(/^[pnbrqk]$/);
  await expect(frame.locator(`.board > .sq[data-square="${best!.slice(0, 2)}"]`)).toHaveAttribute("data-piece", "");
  await expect(frame.locator(`.board > .sq[data-square="${best!.slice(2, 4)}"]`)).toHaveAttribute("data-piece", blackPieces[best!.slice(0, 2)]!);
  await idle(frame);

  // Run the exact production bridge against a real engine, without a UI test backdoor.
  const source = readFileSync(join(__dirname, "../../ui/src/engine.js"), "utf8");
  const mate = await frame.locator("body").evaluate(async (_element, source) => {
    const urls = (window as any).freechessEngine.assets as string[];
    const js = urls.find(url => /\.js(?:[?#]|$)/.test(url));
    const wasm = urls.find(url => /\.wasm(?:[?#]|$)/.test(url));
    if (!js || !wasm) throw new Error("Production engine asset requests were not observed");
    return new Promise<{ best_move?: string; error?: string }>((resolve, reject) => {
      let first = true;
      const timer = setTimeout(() => reject(new Error("Real engine tactical search timed out")), 20_000);
      const bridge = new Function("dioxus", `return (async () => {${source}\n})();`);
      Promise.resolve(bridge({
        recv: () => first
          ? (first = false, Promise.resolve([js, wasm, "position fen 7k/6pp/5KQ1/8/8/8/8/8 w - - 0 1", 20]))
          : new Promise(() => {}),
        send: (result: { best_move?: string; error?: string }) => {
          clearTimeout(timer);
          resolve(result);
        },
      })).catch(error => { clearTimeout(timer); reject(error); });
    });
  }, source);
  expect(mate.error).toBeNull();
  expect(mate.best_move).toBe("g6g7");
  await idle(frame);
});

test("difficulty configures every real search and changing it cancels the previous search", async ({ page }) => {
  const frame = await openComputer(page);
  const difficulty = frame.locator("#engine-difficulty");
  await expect(difficulty).toHaveAccessibleName("Difficulty");
  await expect(difficulty).toHaveValue("5");
  const blackPieces = await frame.locator(".board > .sq").evaluateAll(squares => Object.fromEntries(
    squares.map(square => [square.getAttribute("data-square"), square.getAttribute("data-piece")]),
  ));
  for (const skill of [5, 0, 10, 20]) {
    await difficulty.selectOption(String(skill));
    const before = await observed(frame);
    await playMove(frame, "e2", "e4");
    await expect(frame.locator(".moves .mv")).toHaveCount(2);
    await idle(frame);
    const state = await observed(frame);
    expect(state.created).toBe(before.created + 1);
    const commands = state.commands.filter(entry => entry.worker === state.created).map(entry => entry.command);
    const configured = commands.indexOf(`setoption name Skill Level value ${skill}`);
    expect(configured).toBeGreaterThanOrEqual(0);
    expect(commands.findIndex(command => command.startsWith("go "))).toBeGreaterThan(configured);
    const best = state.lines.slice(before.lines.length).find(line => line.startsWith("bestmove "))?.split(/\s+/)[1];
    expect(best).toMatch(/^[a-h][1-8][a-h][1-8][qrbn]?$/);
    expect(blackPieces[best!.slice(0, 2)]).toMatch(/^[pnbrqk]$/);
    await expect(frame.locator(`.board > .sq[data-square="${best!.slice(0, 2)}"]`)).toHaveAttribute("data-piece", "");
    await expect(frame.locator(`.board > .sq[data-square="${best!.slice(2, 4)}"]`)).toHaveAttribute("data-piece", blackPieces[best!.slice(0, 2)]!);
    await frame.getByRole("button", { name: "New game", exact: true }).click();
    await expect(frame.locator(".moves .mv")).toHaveCount(0);
    await expect(difficulty).toHaveValue(String(skill));
  }

  const previous = (await observed(frame)).created;
  await playMove(frame, "e2", "e4");
  await expect.poll(async () => (await observed(frame)).commands.some(entry =>
    entry.worker > previous && entry.command.startsWith("go ")),
  { timeout: 20_000 }).toBe(true);
  await expect(frame.locator(".moves .mv")).toHaveCount(1);
  const canceled = (await observed(frame)).created;
  await difficulty.selectOption("0");
  await expect.poll(async () => (await observed(frame)).created, { timeout: 20_000 }).toBeGreaterThan(canceled);
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await idle(frame);
  const state = await observed(frame);
  const commands = state.commands.filter(entry => entry.worker === state.created).map(entry => entry.command);
  expect(commands).toContain("setoption name Skill Level value 0");
  expect(commands.indexOf("setoption name Skill Level value 0")).toBeLessThan(commands.findIndex(command => command.startsWith("go ")));
  expect(state.terminated).toBe(state.created);
  await page.waitForTimeout(1_000);
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await frame.getByRole("button", { name: "New game", exact: true }).click();
  await expect(difficulty).toHaveValue("0");
});

test("undo, restart and leaving during search discard stale results and release workers", async ({ page }) => {
  let frame = await openComputer(page);
  for (const action of ["Undo turn", "New game", "leave", "New game"]) {
    const before = (await observed(frame)).created;
    await playMove(frame, "e2", "e4");
    await expect.poll(async () => (await observed(frame)).created, { timeout: 20_000 }).toBeGreaterThan(before);
    await expect(frame.locator("#engine-status")).toHaveAttribute("data-status", "thinking");
    if (action === "leave") {
      await frame.locator('.brand[title="Home"]').click();
      await frame.getByRole("button", { name: "Play computer", exact: true }).click();
      frame = app(page);
    } else {
      await frame.getByRole("button", { name: action, exact: true }).click();
    }
    await expect(frame.locator(".moves .mv")).toHaveCount(0);
    await expect(frame.locator('.board > .sq[data-square="e2"]')).toHaveAttribute("data-piece", "P");
    await expect(frame.locator('.board > .sq[data-square="e4"]')).toHaveAttribute("data-piece", "");
    await idle(frame);
  }
  // Outlast the canceled 750ms searches; a late response must not add a move.
  await page.waitForTimeout(1_000);
  await expect(frame.locator(".moves .mv")).toHaveCount(0);
  const state = await observed(frame);
  expect(state.created).toBeGreaterThanOrEqual(4);
  expect(state.terminated).toBe(state.created);
  await idle(frame);
});

test("missing engine assets show a recoverable error instead of a substitute opponent", async ({ page }) => {
  let blocked = true;
  await page.route(/stockfish/i, route => blocked ? route.abort("failed") : route.continue());
  const frame = await openComputer(page);
  await playMove(frame, "e2", "e4");
  await expect(frame.locator("#engine-status")).toHaveAttribute("data-status", "error");
  await expect(frame.locator("#engine-error")).toContainText("Stockfish unavailable");
  await expect(frame.locator("#retry-stockfish")).toBeVisible();
  await page.waitForTimeout(1_000);
  await expect(frame.locator(".moves .mv")).toHaveCount(1);
  expect((await observed(frame)).lines.some(line => /^bestmove /.test(line))).toBe(false);
  await idle(frame);
  blocked = false;
  await frame.getByRole("button", { name: "Retry Stockfish", exact: true }).click();
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await expect(frame.locator("#engine-error")).toBeHidden();
  expect((await observed(frame)).lines.some(line => /^id name Stockfish 19/i.test(line))).toBe(true);
  await idle(frame);
});
