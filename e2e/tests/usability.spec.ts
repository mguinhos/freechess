import { test, expect } from "@playwright/test";
import { writeFileSync } from "fs";
import { app, appUrl, GATEWAY_PORT, PEER2_PORT, playMove, waitForAccount } from "./helpers";

test("computer play supports click, keyboard, drag, undo and mute", async ({ page }) => {
  await page.addInitScript(() => {
    const Original = window.AudioContext;
    const audio = { contexts: [] as AudioContext[], tones: 0 };
    (window as any).freechessAudio = audio;
    window.AudioContext = class extends Original {
      constructor(options?: AudioContextOptions) {
        super(options);
        audio.contexts.push(this);
      }
      createOscillator() {
        audio.tones++;
        return super.createOscillator();
      }
    };
  });
  await page.goto(appUrl(GATEWAY_PORT));
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  await expect(frame.locator(".board > .sq")).toHaveCount(64);
  await expect(frame.locator('.sq[data-square="e2"]')).toHaveAttribute("data-piece", "P");

  await playMove(frame, "e2", "e4");
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await expect(frame.locator('.sq[data-square="e4"]')).toHaveAttribute("data-piece", "P");
  const content = await frame.locator("body").elementHandle();
  expect(await content!.evaluate(() => (window as any).freechessAudio.tones)).toBeGreaterThan(0);
  expect(await content!.evaluate(() => (window as any).freechessAudio.contexts[0].state)).toBe("running");

  await frame.getByRole("button", { name: "Undo turn", exact: true }).click();
  await expect(frame.locator(".moves .mv")).toHaveCount(0);
  await frame.locator('.sq[data-square="e2"]').focus();
  await frame.locator('.sq[data-square="e2"]').press("Enter");
  await frame.locator('.sq[data-square="e2"]').press("ArrowUp");
  await frame.locator('.sq[data-square="e3"]').press("ArrowUp");
  await frame.locator('.sq[data-square="e4"]').press("Enter");
  await expect(frame.locator(".moves .mv")).toHaveCount(2);

  await frame.getByRole("button", { name: "Mute sounds", exact: true }).click();
  const before = await content!.evaluate(() => (window as any).freechessAudio.tones);
  await frame.locator('.sq[data-square="g1"]').dragTo(frame.locator('.sq[data-square="f3"]'));
  await expect(frame.locator(".moves .mv")).toHaveCount(4);
  expect(await content!.evaluate(() => (window as any).freechessAudio.tones)).toBe(before);
  await frame.getByRole("button", { name: "Enable sounds", exact: true }).click();
  await frame.getByRole("button", { name: "New game", exact: true }).click();
  await expect(frame.locator(".moves .mv")).toHaveCount(0);
});

test("drag preview uses a detached transparent bitmap and keeps moves legal", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const probe = { images: [] as any[], started: 0, ended: 0, pointer: { x: 0, y: 0 } };
    (window as any).freechessDragPreview = probe;
    document.addEventListener("dragstart", event => {
      const square = event.target instanceof Element ? event.target.closest(".sq") : null;
      if (square) {
        probe.started++;
        const rect = square.getBoundingClientRect();
        probe.pointer = { x: event.clientX - rect.left, y: event.clientY - rect.top };
      }
    }, true);
    document.addEventListener("dragend", () => probe.ended++, true);
    const native = DataTransfer.prototype.setDragImage;
    DataTransfer.prototype.setDragImage = function (element, x, y) {
      native.call(this, element, x, y);
      const image = element as HTMLImageElement;
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const context = canvas.getContext("2d")!;
      context.drawImage(image, 0, 0);
      const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
      let border = 0, clear = 0, painted = 0;
      const palette = new Map<string, number>();
      for (let i = 0; i < pixels.length; i += 4) {
        const pixel = i / 4, column = pixel % canvas.width, row = Math.floor(pixel / canvas.width);
        if (!pixels[i + 3]) clear++;
        else {
          painted++;
          if (!column || column === canvas.width - 1 || !row || row === canvas.height - 1) border++;
          if (pixels[i + 3] === 255) {
            const rgb = [...pixels.slice(i, i + 3)].join(",");
            palette.set(rgb, (palette.get(rgb) ?? 0) + 1);
          }
        }
      }
      probe.images.push({
        tag: image.tagName, connected: image.isConnected, parent: image.parentElement?.tagName ?? null,
        ready: image.complete && image.naturalWidth > 0,
        width: canvas.width, height: canvas.height, x, y, pointer: probe.pointer,
        border, clear, painted, colors: [...palette].sort((a, b) => b[1] - a[1]).slice(0, 8).map(([rgb]) => rgb),
        png: canvas.toDataURL("image/png"),
      });
    };
  });
  await page.goto(appUrl(PEER2_PORT));
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  const initial = frame.locator('.board > .sq[data-square="g1"]');
  await initial.scrollIntoViewIfNeeded();
  const initialBox = (await initial.boundingBox())!;
  await page.mouse.move(initialBox.x + initialBox.width / 2, initialBox.y + initialBox.height / 2);
  await page.mouse.down();
  try {
    await page.mouse.move(initialBox.x + initialBox.width / 2 + 12, initialBox.y + initialBox.height / 2 - 12, { steps: 4 });
    const probe = await initial.evaluate(() => (window as any).freechessDragPreview);
    expect(probe.started, "The first mouse gesture must reach native dragstart").toBe(1);
    const images = probe.images;
    expect(images, "The first drag must use an explicit decoded image without a preparatory hover or sleep").toHaveLength(1);
    expect(images[0].ready).toBe(true);
  } finally {
    await page.mouse.up();
  }
  await waitForAccount(frame);
  await expect(frame.locator("#theme-button")).toHaveAttribute("data-loaded", "true");
  await frame.locator("#theme-button").click();
  const dialog = frame.getByRole("dialog", { name: "Appearance", exact: true });
  const ids = ["light-square-color", "dark-square-color", "white-piece-color", "black-piece-color"];
  const original = {
    set: await dialog.getByRole("button", { name: "Neo", exact: true }).getAttribute("aria-pressed") === "true" ? "Neo" : "Classic",
    colors: await Promise.all(ids.map(id => dialog.locator(`#${id}`).inputValue())),
  };
  const configure = async (set: string, colors: string[]) => {
    await dialog.getByRole("button", { name: set, exact: true }).click();
    for (let i = 0; i < ids.length; i++) {
      await dialog.locator(`#${ids[i]}`).evaluate((element, value) => {
        const input = element as HTMLInputElement;
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }, colors[i]);
    }
    await expect(dialog.locator("#theme-save-status")).toHaveText("Saved for this account");
    await dialog.press("Escape");
  };
  try {
    for (const [set, fill] of [["Neo", "#e86b91"], ["Classic", "#72cef2"]]) {
      if (!await dialog.isVisible()) await frame.locator("#theme-button").click();
      await configure(set, ["#123456", "#654321", fill, "#4a2612"]);
      const source = frame.locator('.board > .sq[data-square="g1"]');
      for (const destination of ["g3", "outside", "f3"]) {
        await source.scrollIntoViewIfNeeded();
        const before = await source.evaluate(() => {
          const probe = (window as any).freechessDragPreview;
          return { images: probe.images.length, ended: probe.ended };
        });
        const from = (await source.boundingBox())!;
        const target = destination === "outside" ? frame.locator("#engine-status") : frame.locator(`.board > .sq[data-square="${destination}"]`);
        const to = (await target.boundingBox())!;
        await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
        await page.mouse.down();
        try {
          await page.mouse.move(from.x + from.width / 2 + 12, from.y + from.height / 2 - 12, { steps: 4 });
          await expect(source).toHaveAttribute("data-piece", "N");
          const images = await source.evaluate(() => (window as any).freechessDragPreview.images);
          expect(images, "Every native drag must explicitly use its decoded bitmap").toHaveLength(before.images + 1);
          const preview = images[images.length - 1];
          expect({ tag: preview.tag, connected: preview.connected, parent: preview.parent, ready: preview.ready })
            .toEqual({ tag: "IMG", connected: false, parent: null, ready: true });
          expect(Math.abs(preview.width - from.width)).toBeLessThanOrEqual(1);
          expect(Math.abs(preview.height - from.height)).toBeLessThanOrEqual(1);
          expect(Math.abs(preview.x - preview.pointer.x)).toBeLessThanOrEqual(1);
          expect(Math.abs(preview.y - preview.pointer.y)).toBeLessThanOrEqual(1);
          expect(preview.border).toBe(0);
          expect(preview.clear).toBeGreaterThan(preview.width * preview.height * 0.2);
          expect(preview.painted).toBeGreaterThan(preview.width * preview.height * 0.1);
          expect(preview.colors).toContain([1, 3, 5].map(i => parseInt(fill.slice(i, i + 2), 16)).join(","));
          if (destination === "f3") {
            const path = testInfo.outputPath(`piece-drag-preview-${set}.png`);
            writeFileSync(path, Buffer.from(preview.png.split(",")[1], "base64"));
            await testInfo.attach(`Exact native drag bitmap: ${set}`, { path, contentType: "image/png" });
          }
          await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 8 });
        } finally {
          await page.mouse.up();
        }
        if (destination === "f3") {
          await expect(source).toHaveAttribute("data-piece", "");
          await expect(target).toHaveAttribute("data-piece", "N");
          await expect(frame.locator(".moves .mv").first()).toHaveText("Nf3");
        } else {
          // Legal drops remove the dragged node before dragend can bubble.
          await expect.poll(() => source.evaluate(() => (window as any).freechessDragPreview.ended)).toBe(before.ended + 1);
          await expect(source).toHaveAttribute("data-piece", "N");
          await expect(frame.locator(".moves .mv")).toHaveCount(0);
        }
      }
      await frame.getByRole("button", { name: "New game", exact: true }).click();
    }
  } finally {
    if (!await dialog.isVisible()) await frame.locator("#theme-button").click();
    await configure(original.set, original.colors);
  }
});

test("mobile computer board fits and works without a connected node", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.routeWebSocket("**/v1/contract/command*", socket => socket.close());
  await page.goto(appUrl(GATEWAY_PORT));
  const frame = app(page);
  await frame.getByRole("button", { name: "Play computer", exact: true }).click();
  await playMove(frame, "e2", "e4");
  await expect(frame.locator(".moves .mv")).toHaveCount(2);
  await expect(frame.locator("#nickname-prompt")).toBeHidden();
  const body = await frame.locator("body").elementHandle();
  expect(await body!.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
