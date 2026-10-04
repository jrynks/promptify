import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { chromium as playwrightChromium, expect } from "@playwright/test";
import { focusNativeTest } from "./native-test-focus.mjs";
import { openNativeTestPage } from "./native-test-page.mjs";

if (process.argv.length < 3) {
  throw new Error("Usage: node scripts/test-generated-paste.mjs <built-promptify-cli>");
}
const executable = resolve(process.argv[2]);
let browserOptions = process.env.PROMPTIFY_TEST_BROWSER ? { executablePath: process.env.PROMPTIFY_TEST_BROWSER } : {};
const features = [];
for (let index = 3; index < process.argv.length; index++) {
  if (process.argv[index] === "--disable-features" && process.argv[index + 1]) features.push(process.argv[++index]);
  else throw new Error(`Unknown native test option: ${process.argv[index]}`);
}
if (features.length) browserOptions.args = [...(browserOptions.args ?? []), `--disable-features=${features.join(",")}`];
if (process.env.PROMPTIFY_TEST_FEATURES) browserOptions.args = [...(browserOptions.args ?? []), `--disable-features=${process.env.PROMPTIFY_TEST_FEATURES}`];
const browser = await playwrightChromium.launch({ headless: false, ...browserOptions });
let closePage;
try {
  const title = `Promptify paste test ${randomUUID()}`;
  const fixture = await openNativeTestPage(browser, title, "Generated native target");
  const page = fixture.page;
  closePage = fixture.close;
  const field = page.getByRole("textbox", { name: "Generated native target" });
  await field.focus();
  await focusNativeTest(page, title);
  const report = await new Promise((accept, reject) => {
    const child = spawn(executable, ["generated-paste-smoke-test", title, "Explain a heat pump to a beginner"], {
      windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PROMPTIFY_NATIVE_TEST_ACK: "1" },
    });
    void focusNativeTest(page, title).catch((error) => {
      child.kill();
      reject(error);
    });
    let output = "";
    let errors = "";
    let observing = false;
    child.stdout.on("data", (chunk) => {
      output += chunk;
      if (!observing && output.includes("\n")) {
        observing = true;
        try {
          const report = JSON.parse(output);
          void expect(field).toHaveValue(report.outcome.text, { timeout: 5000 }).then(
            () => child.stdin.end("observed\n"),
            (error) => { child.kill(); reject(error); },
          );
        } catch (error) {
          child.kill();
          reject(error);
        }
      }
    });
    child.stderr.on("data", (chunk) => { errors += chunk; });
    const timeout = setTimeout(() => {
      child.kill();
      reject(new Error(`Generated native test timed out.\n${errors}`));
    }, 90000);
    child.once("error", (error) => { clearTimeout(timeout); reject(error); });
    child.once("exit", (code) => {
      clearTimeout(timeout);
      if (code !== 0) return reject(new Error(`Generated native test failed (${code}).\n${errors}`));
      try { accept(JSON.parse(output)); }
      catch (error) { reject(new Error(`Invalid native report: ${String(error)}`)); }
    });
  });
  if (report.outcome?.kind !== "inserted" || report.delivery !== "sent_unverified" || report.routing?.auto_paste !== true) {
    throw new Error("Expected adaptive dispatch with a confirmed native destination");
  }
  const text = report.outcome.text;
  if (!text.includes("Step 1:") || !text.includes("Loop:") || !text.includes("Done when:")) {
    throw new Error("Generated text did not retain the required graph");
  }
  await expect(field).toHaveValue(text, { timeout: 5000 });
  console.log(`PASS: real adaptive model generation reached the native test field exactly on ${process.platform}.`);
} finally {
  try { await browser.close(); }
  finally { if (closePage) await closePage(); }
}
