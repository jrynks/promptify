import { defineConfig } from "@playwright/test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  testDir: "./tests",
  testMatch: "onboarding.spec.ts",
  workers: 1,
  use: {
    baseURL: "http://127.0.0.1:1429",
    viewport: { width: 960, height: 760 },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: `"${process.execPath}" "${join(root, "node_modules", "vite", "bin", "vite.js")}" preview --host 127.0.0.1 --port 1429 --strictPort`,
    cwd: root,
    url: "http://127.0.0.1:1429",
    reuseExistingServer: false,
  },
});
