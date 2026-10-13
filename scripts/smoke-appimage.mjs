// Run under xvfb-run and dbus-run-session; inspect the real packaged WebKit UI,
// not a browser preview or the separately packaged CLI.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { closeSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

async function inspect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    const timeout = setTimeout(() => finish(new Error("WebKit inspection timed out")), 5000);
    function finish(error, value) {
      clearTimeout(timeout);
      socket.close();
      if (error) reject(error);
      else resolve(value);
    }
    socket.onerror = () => finish(new Error(`Could not connect to ${url}`));
    socket.onmessage = ({ data }) => {
      const message = JSON.parse(data);
      if (message.method === "Target.targetCreated") {
        socket.send(JSON.stringify({
          id: 1, method: "Target.sendMessageToTarget",
          params: {
            targetId: message.params.targetInfo.targetId,
            message: JSON.stringify({
              id: 2, method: "Runtime.evaluate",
              params: {
                expression: `(() => {
                  if (location.pathname === '/overlay.html' && !document.getElementById('root')?.childElementCount) {
                    window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {event:'overlay-event',payload:{type:'stage',stage:'generating'}})
                      .catch(error => { window.appimageSmokeError = String(error); });
                  }
                  return JSON.stringify({url:location.href,children:document.getElementById('root')?.childElementCount,text:document.body.textContent,error:window.appimageSmokeError});
                })()`,
                returnByValue: true,
              },
            }),
          },
        }));
      } else if (message.method === "Target.dispatchMessageFromTarget") {
        const response = JSON.parse(message.params.message);
        if (response.id === 2) {
          if (response.error || response.result?.wasThrown) finish(new Error(JSON.stringify(response)));
          else finish(null, JSON.parse(response.result.result.value));
        }
      }
    };
  });
}

async function smoke(image, scratch) {
  mkdirSync(scratch, { recursive: true });
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  const logPath = join(scratch, "gui.log");
  const log = openSync(logPath, "w");
  const env = {
    ...process.env, APPIMAGE_EXTRACT_AND_RUN: "1",
    GDK_BACKEND: "x11", XDG_SESSION_TYPE: "x11",
    XDG_DATA_HOME: join(scratch, "xdg-data"),
    XDG_CONFIG_HOME: join(scratch, "xdg-config"),
    XDG_CACHE_HOME: join(scratch, "xdg-cache"),
    PROMPTIFY_DATA_DIR: join(scratch, "data"),
    PROMPTIFY_MODELS_DIR: join(scratch, "models"),
    WEBKIT_INSPECTOR_HTTP_SERVER: `127.0.0.1:${port}`,
  };
  // Do not let developer workarounds hide the packaging regression.
  for (const name of ["LD_PRELOAD", "WAYLAND_DISPLAY", "WEBKIT_DISABLE_DMABUF_RENDERER", "WEBKIT_DISABLE_COMPOSITING_MODE", "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS"]) delete env[name];
  const app = spawn(image, [], { env, stdio: ["ignore", log, log] });
  const exited = new Promise((resolve) => {
    app.once("exit", resolve);
    app.once("error", resolve);
  });
  let spawnError;
  app.on("error", (error) => { spawnError = error; });
  try {
    const deadline = Date.now() + 30_000;
    let pages = [];
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      assert.equal(app.exitCode, null, `AppImage exited before rendering (code ${app.exitCode})`);
      // The inspector socket does not exist until WebKit has initialized.
      const response = await fetch(`http://127.0.0.1:${port}`, { signal: AbortSignal.timeout(1000) }).catch((error) => {
        if (error.cause?.code !== "ECONNREFUSED") throw error;
        return null;
      });
      if (response) {
        assert.ok(response.ok, `WebKit inspector HTTP status ${response.status}`);
        const html = await response.text();
        const sockets = [...html.matchAll(/\/socket\/\d+\/\d+\/WebPage/g)].map((match) => match[0]);
        pages = await Promise.all(sockets.map((path) => inspect(`ws://127.0.0.1:${port}${path}`)));
        const settings = pages.find((page) => page.url === "tauri://localhost" || page.url === "tauri://localhost/");
        const overlay = pages.find((page) => page.url === "tauri://localhost/overlay.html");
        assert.ok(!overlay?.error, `Overlay IPC failed: ${overlay?.error}`);
        if (settings?.children > 0 && overlay?.children > 0 && overlay.text.includes("Writing your prompt") && settings.text.includes("Speech recognition") && settings.text.includes("Welcome to Promptify")) {
          console.log(JSON.stringify({ settingsRendered: true, onboardingRendered: true, overlayRendered: true }));
          return;
        }
      }
      await delay(250);
    }
    throw new Error(`Packaged UI did not render onboarding and overlay: ${JSON.stringify(pages)}`);
  } catch (error) {
    throw new Error(`${error.message}\nAppImage log:\n${readFileSync(logPath, "utf8").slice(-8192)}`, { cause: error });
  } finally {
    app.kill("SIGTERM");
    await Promise.race([exited, delay(5000)]);
    if (app.exitCode === null && app.signalCode === null) {
      app.kill("SIGKILL");
      await exited;
    }
    closeSync(log);
  }
}

assert.equal(process.argv.length, 4, "Usage: node scripts/smoke-appimage.mjs <AppImage> <scratch-directory>");
await smoke(resolve(process.argv[2]), resolve(process.argv[3]));
