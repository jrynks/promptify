import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { _electron as electron, expect } from "@playwright/test";
import { focusNativeTest } from "./native-test-focus.mjs";

if (process.platform !== "win32" || process.argv.length < 4 || process.argv.length > 5 || (process.argv.length === 5 && !["--acceptance", "--generated"].includes(process.argv[4]))) {
  throw new Error("Usage on an interactive Windows desktop: node scripts/test-native-vscode.mjs <built-promptify-cli> <Code.exe> [--acceptance|--generated]");
}
const executable = resolve(process.argv[2]);
const attempts = process.argv[4] === "--acceptance" ? 20 : 1;
const generated = process.argv[4] === "--generated";
const root = await mkdtemp(join(tmpdir(), "promptify-vscode-native-"));
const title = `Promptify paste test ${randomUUID()}`;
const file = join(root, `${title}.txt`);
await writeFile(file, "replace this selection");
const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;
let app;
try {
  app = await electron.launch({
    executablePath: resolve(process.argv[3]),
    args: ["--user-data-dir", join(root, "profile"), "--disable-extensions", "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust", "--new-window", file],
    env,
    timeout: 60000,
  });
  const page = await app.firstWindow();
  await expect(page.locator(".monaco-editor .view-lines").first()).toBeVisible({ timeout: 60000 });
  const input = page.locator(".monaco-editor [role='textbox']").first();
  const readText = () => page.locator(".monaco-editor .view-lines").first().evaluate((element) =>
    Array.from(element.querySelectorAll(".view-line"), (line) => line.textContent.replaceAll("\u00a0", " ").replaceAll("\u200b", "")).join("\n"));
  for (let attempt = 0; attempt < attempts; attempt++) {
  for (const text of generated ? ["Explain a heat pump to a beginner"] : [
    "Native insertion into VS Code.",
    "Step 1: Explain a heat pump.\nStep 2: Check the explanation.\nDone when: accurate and clear.",
    "Unicode: \u00e9 \u65e5\u672c\u8a9e \ud83d\ude80",
  ]) {
    await focusNativeTest(page, title);
    await input.focus();
    await page.keyboard.press("Control+A");
    let expected = text;
    await new Promise((accept, reject) => {
      const child = spawn(executable, [generated ? "generated-paste-smoke-test" : "paste-smoke-test", title, text], {
        windowsHide: true, stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, PROMPTIFY_NATIVE_TEST_ACK: "1" },
      });
      let output = "";
      let errors = "";
      let observing = false;
      const timeout = setTimeout(() => { child.kill(); reject(new Error(`Native editor paste timed out.\n${errors}`)); }, generated ? 90000 : 15000);
      void focusNativeTest(page, title).catch((error) => { child.kill(); reject(error); });
      child.stdout.on("data", (chunk) => {
        output += chunk;
        if (!observing && (generated ? output.includes("\n") : output.includes("Native paste dispatched"))) {
          observing = true;
          if (generated) {
            try {
              const report = JSON.parse(output);
              if (report.outcome?.kind !== "inserted" || report.delivery !== "sent_unverified" || report.routing?.auto_paste !== true) {
                throw new Error("Adaptive insertion did not confirm the native editor destination.");
              }
              expected = report.outcome.text;
              if (!expected.includes("Step 1:") || !expected.includes("Loop:") || !expected.includes("Done when:")) {
                throw new Error("Generated text did not retain the required graph.");
              }
            } catch (error) {
              child.kill();
              reject(error);
              return;
            }
          }
          void expect.poll(readText, { timeout: 5000 }).toBe(expected).then(
            () => child.stdin.end("observed\n"),
            (error) => { child.kill(); reject(error); },
          );
        }
      });
      child.stderr.on("data", (chunk) => { errors += chunk; });
      child.once("error", (error) => { clearTimeout(timeout); reject(error); });
      child.once("exit", (code) => {
        clearTimeout(timeout);
        if (code === 0) accept();
        else reject(new Error(`Native editor paste failed (${code}).\n${errors}`));
      });
    });
    await expect.poll(readText).toBe(expected);
  }
  }
  await page.keyboard.press("Control+S");
  console.log(generated
    ? "PASS: real adaptive generated prompt reached an isolated installed VS Code editor exactly. This does not certify authenticated chat fields."
    : `PASS: ${attempts * 3} native single-line, multiline and Unicode replacements into an isolated installed VS Code editor. This does not certify authenticated chat fields.`);
} finally {
  if (app) await app.close();
  await rm(root, { recursive: true, force: true });
}
