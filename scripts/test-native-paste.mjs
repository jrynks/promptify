import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { chromium, expect } from "@playwright/test";

if (!["win32", "darwin", "linux"].includes(process.platform)) {
  throw new Error("Run this smoke test in an interactive Windows, macOS, or Linux desktop session.");
}
if (process.argv.length !== 3) {
  throw new Error("Usage: node scripts/test-native-paste.mjs <path-to-built-promptify-cli>");
}
const executable = resolve(process.argv[2]);
const browser = await chromium.launch({ headless: false });
try {
  const page = await browser.newPage();
  const title = `Promptify paste test ${randomUUID()}`;
  await page.goto(`data:text/html,${encodeURIComponent(`<title>${title}</title><label>Native paste target<textarea aria-label="Native paste target" rows="20" cols="80"></textarea></label>`)}`);
  for (const text of ["Native automatic paste works.", "Step 1: Explain a heat pump.\nStep 2: Check the explanation.\nDone when: accurate and clear."]) {
    const field = page.getByRole("textbox", { name: "Native paste target" });
    await field.fill("");
    await page.bringToFront();
    await field.focus();
    await new Promise((accept, reject) => {
      const child = spawn(executable, ["paste-smoke-test", title, text], { windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
      let output = "";
      child.stdout.on("data", (chunk) => { output += chunk; });
      child.stderr.on("data", (chunk) => { output += chunk; });
      const timeout = setTimeout(() => {
        child.kill();
        reject(new Error(`Native paste timed out.\n${output}`));
      }, 15000);
      child.once("error", (error) => { clearTimeout(timeout); reject(error); });
      child.once("exit", (code) => {
        clearTimeout(timeout);
        if (code === 0) accept();
        else reject(new Error(`Native paste failed (exit ${code}). On macOS grant Accessibility to the CLI/launcher; keep the test field focused.\n${output}`));
      });
    });
    // This observes OS-injected paste; it never supplies the expected text through Playwright.
    await expect(field).toHaveValue(text, { timeout: 5000 });
  }
  console.log(`PASS: real native paste inserted both single-line and multiline text on ${process.platform}.`);
} finally {
  await browser.close();
}
