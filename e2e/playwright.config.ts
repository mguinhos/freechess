import { defineConfig } from "@playwright/test";

// The two nodes are started by ../scripts/nodes.sh before the suite runs.
// Their WebSocket ports are what distinguishes them; the app derives its node
// connection from the page's own origin, so simply loading the app from a
// different port makes the browser talk to a different node.
export default defineConfig({
  testDir: "./tests",
  // Serial: these tests publish to a shared network and observe each other's
  // effects, so running them in parallel would have them stepping on one
  // another's lobby state.
  fullyParallel: false,
  workers: 1,
  // A P2P round trip is not instant; give assertions room without making a
  // real failure take forever to surface.
  timeout: 180_000,
  // A node reaching a contract for the first time has to fetch it across the
  // network — the webapp alone is ~400KB. 45s was enough for a warm node and
  // not for a cold one, which showed up as the first test against a given node
  // timing out while later ones passed.
  expect: { timeout: 90_000 },
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    // Each context gets its own storage, and therefore its own account —
    // which is exactly what "two different players" needs.
    storageState: undefined,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
