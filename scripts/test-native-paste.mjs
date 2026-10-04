import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { chromium as playwrightChromium, expect } from "@playwright/test";
import { focusNativeTest, inspectNativeTestClipboard } from "./native-test-focus.mjs";
import { openNativeTestPage } from "./native-test-page.mjs";

if (!["win32", "darwin", "linux"].includes(process.platform)) {
  throw new Error("Run this smoke test in an interactive Windows, macOS, or Linux desktop session.");
}
const options = process.argv.slice(3);
let acceptance = false;
const features = [];
for (let index = 0; index < options.length; index++) {
  if (options[index] === "--acceptance" && !acceptance) acceptance = true;
  else if (options[index] === "--disable-features" && options[index + 1]) features.push(options[++index]);
  else throw new Error("Usage: node scripts/test-native-paste.mjs <path-to-built-promptify-cli> [--acceptance] [--disable-features <feature>]");
}
const executable = resolve(process.argv[2]);
const attempts = acceptance ? 20 : 1;
let browserOptions = process.env.PROMPTIFY_TEST_BROWSER ? { executablePath: process.env.PROMPTIFY_TEST_BROWSER } : {};
if (features.length) browserOptions.args = [...(browserOptions.args ?? []), `--disable-features=${features.join(",")}`];
if (process.env.PROMPTIFY_TEST_FEATURES) browserOptions.args = [...(browserOptions.args ?? []), `--disable-features=${process.env.PROMPTIFY_TEST_FEATURES}`];
const browser = await playwrightChromium.launch({ headless: false, ...browserOptions });
let closePage;
try {
  const title = `Promptify paste test ${randomUUID()}`;
  const fixture = await openNativeTestPage(browser, title, "Native paste target");
  const page = fixture.page;
  closePage = fixture.close;
  await page.evaluate(() => {
    window.nativePasteEvents = [];
    window.nativeInputEvents = [];
    document.addEventListener("paste", (event) => {
      window.nativePasteEvents.push({ time: performance.now(), length: event.clipboardData?.getData("text/plain").length ?? -1 });
    });
    document.addEventListener("beforeinput", (event) => {
      window.nativeInputEvents.push({ time: performance.now(), type: event.inputType });
    });
  });
  let verified = 0;
  for (let attempt = 0; attempt < attempts; attempt++) {
  for (const text of ["Native automatic paste works.", "Step 1: Explain a heat pump.\nStep 2: Check the explanation.\nDone when: accurate and clear.", "Unicode: \u00e9 \u65e5\u672c\u8a9e \ud83d\ude80"]) {
    const field = page.getByRole("textbox", { name: "Native paste target" });
    const prefix = "Existing prefix: ";
    const suffix = " :existing suffix";
    await field.fill(`${prefix}replace this selection${suffix}`);
    await focusNativeTest(page, title);
    await field.focus();
    await field.evaluate((element, boundaries) => {
      if (!(element instanceof HTMLTextAreaElement)) throw new Error("Expected textarea");
      element.setSelectionRange(boundaries.start, boundaries.end);
    }, { start: prefix.length, end: prefix.length + "replace this selection".length });
    await new Promise((accept, reject) => {
      const child = spawn(executable, ["paste-smoke-test", title, text], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, PROMPTIFY_NATIVE_TEST_ACK: "1" } });
      void focusNativeTest(page, title).catch((error) => {
        child.kill();
        reject(error);
      });
      let output = "";
      let observing = false;
      child.stdout.on("data", (chunk) => {
        output += chunk;
        if (!observing && output.includes("Native paste dispatched")) {
          observing = true;
          void expect(field).toHaveValue(`${prefix}${text}${suffix}`, { timeout: 5000 }).then(
            () => child.stdin.end("observed\n"),
            async (error) => {
              try {
                const diagnostic = await page.evaluate(() => ({ paste: window.nativePasteEvents, input: window.nativeInputEvents }));
                const clipboard = await inspectNativeTestClipboard(text);
                reject(new Error(`Native observation failed at round ${attempt + 1}: ${JSON.stringify({ ...diagnostic, clipboard })}`, { cause: error }));
              } catch (diagnosticError) {
                reject(new Error(`Native observation and diagnostic failed: ${String(diagnosticError)}`, { cause: error }));
              } finally {
                child.kill();
              }
            },
          );
        }
      });
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
    try {
      await expect(field).toHaveValue(`${prefix}${text}${suffix}`, { timeout: 5000 });
    } catch (error) {
      const events = await page.evaluate(() => window.nativePasteEvents);
      const input = await page.evaluate(() => window.nativeInputEvents);
      throw new Error(`Native replacement failed at round ${attempt + 1}; expected ${text.length} characters. Paste-event timing/length: ${JSON.stringify(events)}. Input-event types: ${JSON.stringify(input)}.`, { cause: error });
    }
    verified++;
  }
  }
  console.log(`PASS: ${verified} exact native selection replacements on ${process.platform}; ${attempts} attempts each for single-line, multiline and Unicode text.`);
} finally {
  try { await browser.close(); }
  finally { if (closePage) await closePage(); }
}
