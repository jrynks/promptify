import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readdirSync, renameSync, rmSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function bundledWaylandLibraries(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return bundledWaylandLibraries(path);
    return /^libwayland-.*\.so(?:\..*)?$/.test(entry.name) ? [path] : [];
  });
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 8 * 1024 * 1024, ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} failed (${result.status ?? result.signal}):\n${result.stderr?.slice(-8192)}\n${result.stdout?.slice(-8192)}`);
}

// Tauri's linuxdeploy bundles Jammy's Wayland libraries, but not the host's Mesa
// driver. New Mesa drivers need newer Wayland symbols even on X11.
export function repairAppImage(image, plugin = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "tauri", "linuxdeploy-plugin-appimage.AppImage")) {
  image = resolve(image);
  const scratch = mkdtempSync(join(dirname(image), ".appimage-"));
  try {
    run(image, ["--appimage-extract"], { cwd: scratch, stdio: ["ignore", "ignore", "pipe"] });
    const payload = join(scratch, "squashfs-root");
    assert.ok(existsSync(join(payload, "AppRun")), "AppImage is missing AppRun");
    const libraries = bundledWaylandLibraries(payload);
    if (!libraries.length) return { removed: 0 };
    for (const library of libraries) rmSync(library);
    assert.ok(existsSync(plugin), `Missing Tauri AppImage output plugin: ${plugin}`);
    const output = join(scratch, "repaired.AppImage");
    run(plugin, ["--appdir", payload], {
      env: { ...process.env, APPIMAGE_EXTRACT_AND_RUN: "1", OUTPUT: output, LDAI_OUTPUT: output, NO_STRIP: "1" },
    });
    assert.ok(existsSync(output) && statSync(output).size > 0, "AppImage repacking produced no output");
    chmodSync(output, statSync(image).mode | 0o111);
    renameSync(output, image);
    console.log(`Repacked ${image} using host Wayland libraries (removed ${libraries.length} bundled files).`);
    return { removed: libraries.length };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv.length, 3, "Usage: node scripts/appimage.mjs <AppImage>");
  repairAppImage(process.argv[2]);
}
