// Inspect real installer payloads, not just the staged sidecar files.
// Requires 7z on Windows; dpkg-deb, rpm, rpm2cpio, cpio and readelf on Linux.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 512 * 1024 * 1024, ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} ${args.join(" ")} failed:\n${result.stderr?.toString()}\n${result.stdout?.toString()}`);
  return result.stdout;
}

function filesUnder(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return filesUnder(path);
    // stat follows AppImage symlinks; directories must not pass as executables.
    return statSync(path).isFile() ? [path] : [];
  });
}

function sameBytes(actual, expected) {
  const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
  assert.equal(hash(actual), hash(expected), `Packaged binary differs from the release build: ${actual}`);
}

export function verifyPayload(directory, release, windows = process.platform === "win32", { exactBytes = true } = {}) {
  const extension = windows ? ".exe" : "";
  const appName = `promptify${extension}`;
  const files = filesUnder(directory);
  const apps = files.filter((path) => path.split(/[\\/]/).at(-1).toLowerCase() === appName);
  assert.equal(apps.length, 1, `Expected exactly one ${appName} in ${directory}, found ${apps.length}`);
  const app = apps[0];
  const siblings = [app, ...["promptify-llm", "promptify-cli"].map((name) => join(dirname(app), `${name}${extension}`))];
  for (const binary of siblings) {
    assert.ok(existsSync(binary) && statSync(binary).isFile() && statSync(binary).size > 0, `Missing executable next to app: ${binary}`);
    const header = readFileSync(binary);
    if (windows) {
      assert.equal(header.subarray(0, 2).toString(), "MZ", `Not a PE executable: ${binary}`);
      const pe = header.readUInt32LE(0x3c);
      assert.equal(header.subarray(pe, pe + 4).toString(), "PE\0\0", `Invalid PE header: ${binary}`);
      assert.equal(header.readUInt16LE(pe + 4), 0x8664, `Not a Windows x64 executable: ${binary}`);
    } else {
      assert.equal(header.subarray(0, 4).toString(), "\x7fELF", `Not an ELF executable: ${binary}`);
      assert.equal(header[4], 2, `Not ELF64: ${binary}`);
      assert.equal(header[5], 1, `Not little-endian ELF: ${binary}`);
      assert.equal(header.readUInt16LE(18), 62, `Not Linux x64: ${binary}`);
      assert.ok(statSync(binary).mode & 0o111, `Executable permission is missing: ${binary}`);
      const versions = run("readelf", ["--version-info", binary]);
      for (const [, major, minor] of versions.matchAll(/\bGLIBC_(\d+)\.(\d+)\b/g)) {
        assert.ok(Number(major) < 2 || (Number(major) === 2 && Number(minor) <= 35), `Requires newer than glibc 2.35: ${binary} (GLIBC_${major}.${minor})`);
      }
    }
  }
  // The app locates the worker using current_exe().parent(), with no target suffix.
  // linuxdeploy may rewrite AppImage ELF RPATHs; its layout and ELF checks still apply.
  if (exactBytes) for (const binary of siblings.slice(1)) sameBytes(binary, join(release, binary.split(/[\\/]/).at(-1)));
  if (windows) {
    for (const dll of ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll"]) {
      const path = join(dirname(app), dll);
      assert.ok(existsSync(path) && statSync(path).size > 0, `Missing app-local runtime: ${path}`);
    }
  }
  console.log(`Verified app, adjacent worker and CLI: ${dirname(app)}`);
  return { app: app.slice(directory.length + 1), adjacentWorker: true, adjacentCli: true, matchesReleaseBuild: exactBytes, architecture: "x64", ...(windows ? { appLocalRuntimes: true } : { maximumGlibc: "2.35" }) };
}

export function findInstallers(bundle, windows = process.platform === "win32") {
  const kinds = windows ? [["nsis", /-setup\.exe$/i]] : [["deb", /\.deb$/], ["rpm", /\.rpm$/], ["appimage", /\.AppImage$/]];
  return kinds.flatMap(([kind, pattern]) => {
    const directory = join(bundle, kind);
    assert.ok(existsSync(directory), `Missing ${kind} installer directory: ${directory}`);
    const paths = readdirSync(directory).filter((name) => pattern.test(name)).map((name) => join(directory, name));
    assert.ok(paths.length > 0, `No ${kind} installers found in ${directory}`);
    return paths.map((path) => ({ kind, path }));
  });
}

export function verifyInstallers() {
  assert.ok(["linux", "win32"].includes(process.platform), "Only Windows and Linux installers are supported");
  const release = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "release");
  const bundle = join(release, "bundle");
  // Scratch payloads stay inside the project/build tree and are always removed.
  mkdirSync(bundle, { recursive: true });
  const scratch = mkdtempSync(join(bundle, ".verify-"));
  const report = {
    success: false,
    timestamp: new Date().toISOString(),
    sourceCommit: process.env.SOURCE_COMMIT ?? run("git", ["rev-parse", "HEAD"], { cwd: root }).trim(),
    platform: process.platform,
    node: process.version,
    rust: run("rustc", ["--version"]).trim(),
    installers: [],
  };
  try {
    const installers = findInstallers(bundle);
    for (const [index, { kind, path }] of installers.entries()) {
      assert.ok(statSync(path).size > 0, `Empty installer: ${path}`);
      const destination = join(scratch, String(index));
      mkdirSync(destination);
      console.log(`Inspecting ${path}`);
      let payload = destination;
      let metadata = "";
      if (kind === "nsis") {
        run("7z", ["x", "-y", `-o${destination}`, path]);
      } else if (kind === "deb") {
        assert.equal(run("dpkg-deb", ["--field", path, "Architecture"]).trim(), "amd64", "Debian package is not amd64");
        metadata = run("dpkg-deb", ["--info", path]);
        console.log(metadata);
        run("dpkg-deb", ["--extract", path, destination]);
      } else if (kind === "rpm") {
        assert.equal(run("rpm", ["-qp", "--queryformat", "%{ARCH}", path]).trim(), "x86_64", "RPM is not x86_64");
        metadata = run("rpm", ["-qp", "--requires", path]);
        console.log(metadata);
        const archive = run("rpm2cpio", [path], { encoding: null });
        run("cpio", ["--extract", "--make-directories", "--no-absolute-filenames"], { cwd: destination, input: archive });
      } else {
        chmodSync(path, statSync(path).mode | 0o111);
        run(path, ["--appimage-extract"], { cwd: destination });
        payload = join(destination, "squashfs-root");
        assert.ok(existsSync(join(payload, "AppRun")), "AppImage is missing AppRun");
      }
      const checks = verifyPayload(payload, release, undefined, { exactBytes: kind !== "appimage" });
      report.installers.push({
        file: path.slice(bundle.length + 1).replaceAll("\\", "/"),
        kind,
        bytes: statSync(path).size,
        sha256: createHash("sha256").update(readFileSync(path)).digest("hex"),
        metadata,
        checks,
      });
    }
    report.success = true;
    console.log(`Verified ${installers.length} installer(s); safe to upload.`);
  } catch (error) {
    report.error = error.stack ?? String(error);
    throw error;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
    writeFileSync(join(bundle, "verification.json"), `${JSON.stringify(report, null, 2)}\n`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  verifyInstallers();
}
