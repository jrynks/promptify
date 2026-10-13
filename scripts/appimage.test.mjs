import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { bundledWaylandLibraries, repairAppImage } from "./appimage.mjs";

function fixture(t) {
  const root = mkdtempSync(join(process.cwd(), ".appimage-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test("finds bundled Wayland libraries and symlinks without following external directories", { skip: process.platform !== "linux" }, (t) => {
  const root = fixture(t);
  const lib = join(root, "usr", "lib");
  mkdirSync(lib, { recursive: true });
  writeFileSync(join(lib, "libwayland-client.so.0.20.0"), "old Wayland");
  symlinkSync("libwayland-client.so.0.20.0", join(lib, "libwayland-client.so.0"));
  symlinkSync("/usr/lib", join(lib, "external"));
  writeFileSync(join(lib, "libwebkit2gtk-4.1.so.0"), "bundled WebKit");
  assert.deepEqual(bundledWaylandLibraries(root).map((path) => path.slice(lib.length + 1)).sort(), [
    "libwayland-client.so.0", "libwayland-client.so.0.20.0",
  ]);
});

test("repacking removes only bundled Wayland and preserves executable mode", { skip: process.platform !== "linux" }, (t) => {
  const root = fixture(t);
  const image = join(root, "Promptify.AppImage");
  writeFileSync(image, `#!/bin/sh
test "$1" = --appimage-extract || exit 2
mkdir -p squashfs-root/usr/lib
touch squashfs-root/AppRun
printf old > squashfs-root/usr/lib/libwayland-client.so.0
printf old > squashfs-root/usr/lib/libwayland-server.so.0
printf webkit > squashfs-root/usr/lib/libwebkit2gtk-4.1.so.0
`);
  chmodSync(image, 0o755);
  const plugin = join(root, "plugin");
  writeFileSync(plugin, `#!/bin/sh
test "$1" = --appdir || exit 2
test ! -e "$2/usr/lib/libwayland-client.so.0" || exit 3
test ! -e "$2/usr/lib/libwayland-server.so.0" || exit 4
test "$(cat "$2/usr/lib/libwebkit2gtk-4.1.so.0")" = webkit || exit 5
printf repaired > "$OUTPUT"
`);
  chmodSync(plugin, 0o755);
  assert.deepEqual(repairAppImage(image, plugin), { removed: 2 });
  assert.equal(readFileSync(image, "utf8"), "repaired");
  assert.equal(statSync(image).mode & 0o777, 0o755);
  assert.deepEqual(readdirSync(root).sort(), ["Promptify.AppImage", "plugin"]);
});

test("failed repacking leaves the original installer intact and removes scratch files", { skip: process.platform !== "linux" }, (t) => {
  const root = fixture(t);
  const image = join(root, "Promptify.AppImage");
  const original = "#!/bin/sh\nmkdir -p squashfs-root/usr/lib\ntouch squashfs-root/AppRun squashfs-root/usr/lib/libwayland-client.so.0\n";
  writeFileSync(image, original);
  chmodSync(image, 0o755);
  assert.throws(() => repairAppImage(image, join(root, "missing-plugin")), /Missing Tauri AppImage output plugin/);
  assert.equal(readFileSync(image, "utf8"), original);
  assert.deepEqual(readdirSync(root), ["Promptify.AppImage"]);
  const plugin = join(root, "failing-plugin");
  writeFileSync(plugin, "#!/bin/sh\nprintf incomplete > \"$OUTPUT\"\necho 'repack failure' >&2\nexit 1\n");
  chmodSync(plugin, 0o755);
  assert.throws(() => repairAppImage(image, plugin), /repack failure/);
  assert.equal(readFileSync(image, "utf8"), original);
  assert.deepEqual(readdirSync(root).sort(), ["Promptify.AppImage", "failing-plugin"]);
});

test("an already-compatible AppImage is unchanged and does not need repacking", { skip: process.platform !== "linux" }, (t) => {
  const root = fixture(t);
  const image = join(root, "Promptify.AppImage");
  const original = "#!/bin/sh\nmkdir squashfs-root\ntouch squashfs-root/AppRun\n";
  writeFileSync(image, original);
  chmodSync(image, 0o755);
  assert.deepEqual(repairAppImage(image, join(root, "missing-plugin")), { removed: 0 });
  assert.equal(readFileSync(image, "utf8"), original);
  assert.deepEqual(readdirSync(root), ["Promptify.AppImage"]);
});
