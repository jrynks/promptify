// Inspect real installer payloads against the exact sidecars staged for Tauri.
// Requires 7z on Windows; dpkg-deb, rpm, rpm2cpio, cpio and readelf on Linux.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, closeSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, readlinkSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const runtimeDlls = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll"];

const OUTPUT_LIMIT = 8 * 1024;

// Keeps errors and verification.json bounded even if a tool prints binary data.
export function excerpt(value, limit = OUTPUT_LIMIT) {
  const text = Buffer.isBuffer(value) ? value.subarray(-limit).toString("utf8") : String(value ?? "");
  return text.length > limit ? `...[${text.length - limit} characters omitted]\n${text.slice(-limit)}` : text;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, ...options });
  if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited ${result.status ?? result.signal}\nstderr:\n${excerpt(result.stderr)}\nstdout:\n${excerpt(result.stdout)}`);
  }
  return result.stdout;
}

// Streams through files and descriptors so large payloads are never buffered by Node.
function runToFile(command, args, outputPath) {
  const output = openSync(outputPath, "w");
  try {
    const result = spawnSync(command, args, { stdio: ["ignore", output, "pipe"], maxBuffer: OUTPUT_LIMIT * 8 });
    if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
    return { status: result.status, signal: result.signal, stderr: excerpt(result.stderr) };
  } finally {
    closeSync(output);
  }
}

function runFromFile(command, args, inputPath, cwd) {
  const input = openSync(inputPath, "r");
  try {
    const result = spawnSync(command, args, { cwd, stdio: [input, "ignore", "pipe"], maxBuffer: OUTPUT_LIMIT * 8 });
    if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
    if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} exited ${result.status ?? result.signal}\nstderr:\n${excerpt(result.stderr)}`);
  } finally {
    closeSync(input);
  }
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

// rpm 4.17's rpm2cpio can exit 1 after writing a complete archive, so the RPM's
// own file manifest and SHA-256 digests decide whether extraction was complete.
export function verifyRpmExtraction(dump, destination) {
  const entries = dump.trim().split("\n").filter(Boolean);
  assert.ok(entries.length > 0, "RPM file manifest is empty");
  let files = 0;
  for (const line of entries) {
    const fields = line.split(" ");
    assert.ok(fields.length >= 11, `Unexpected RPM manifest entry: ${excerpt(line, 512)}`);
    const tail = fields.slice(-10);
    const path = fields.slice(0, -10).join(" ");
    const [size, , digest, mode, , , , , , link] = tail;
    const extracted = join(destination, path.replace(/^\/+/, ""));
    const type = Number.parseInt(mode, 8) & 0o170000;
    if (type === 0o040000) {
      assert.ok(existsSync(extracted) && lstatSync(extracted).isDirectory(), `RPM directory was not extracted: ${path}`);
    } else if (type === 0o120000) {
      assert.equal(readlinkSync(extracted), link, `RPM symlink was not extracted: ${path}`);
    } else {
      assert.equal(type, 0o100000, `Unsupported RPM file type for ${path}`);
      assert.ok(existsSync(extracted) && lstatSync(extracted).isFile(), `RPM file was not extracted: ${path}`);
      assert.equal(statSync(extracted).size, Number(size), `RPM file size differs after extraction: ${path}`);
      assert.equal(sha256(extracted), digest, `RPM file digest differs after extraction: ${path}`);
      files++;
    }
  }
  return files;
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
  assert.ok(existsSync(expected), `Missing staged reference: ${expected}`);
  assert.equal(sha256(actual), sha256(expected), `Packaged file differs from staged sidecar: ${actual}`);
}

export function hostTriple() {
  const triple = run("rustc", ["-vV"]).match(/^host: (\S+)$/m)?.[1];
  assert.ok(triple, "Could not read the Rust host target from `rustc -vV`");
  return triple;
}

// Bounded, GUI-free check that the packaged CLI starts and reads its bundled manifest.
export function smokeCli(cli, scratch) {
  const data = join(scratch, "cli-data");
  const models = join(scratch, "cli-models");
  mkdirSync(models, { recursive: true });
  const env = { ...process.env, PROMPTIFY_DATA_DIR: data, PROMPTIFY_MODELS_DIR: models };
  delete env.PROMPTIFY_LOG;
  const result = spawnSync(cli, ["models"], { encoding: "utf8", timeout: 60_000, env });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `Packaged CLI smoke test failed:\n${result.stderr}\n${result.stdout}`);
  const manifest = JSON.parse(readFileSync(join(root, "crates", "promptify-core", "models", "manifest.json"), "utf8"));
  const ids = manifest.models.map((model) => model.id);
  assert.ok(ids.length > 0, "Bundled model manifest is empty");
  for (const id of ids) {
    assert.match(result.stdout, new RegExp(`^${id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s.*installed=false`, "m"), `Packaged CLI did not list model ${id}`);
  }
  assert.ok(result.stdout.includes(`models folder: ${models}`), "Packaged CLI ignored the isolated models folder");
  return { command: "models", models: ids.length };
}

export function verifyPayload(directory, staging, windows = process.platform === "win32", { exactBytes = true, triple = hostTriple(), smokeScratch } = {}) {
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
  if (exactBytes) {
    for (const name of ["promptify-llm", "promptify-cli"]) {
      sameBytes(join(dirname(app), `${name}${extension}`), join(staging, `${name}-${triple}${extension}`));
    }
  }
  if (windows) {
    for (const dll of runtimeDlls) {
      const path = join(dirname(app), dll);
      assert.ok(existsSync(path) && statSync(path).size > 0, `Missing app-local runtime: ${path}`);
      if (exactBytes) sameBytes(path, join(staging, dll));
    }
  }
  const smoke = smokeScratch ? smokeCli(siblings[2], smokeScratch) : undefined;
  console.log(`Verified app, adjacent worker and CLI: ${dirname(app)}`);
  return { app: app.slice(directory.length + 1), adjacentWorker: true, adjacentCli: true, matchesStagedSidecars: exactBytes, architecture: "x64", ...(smoke && { cliSmoke: smoke }), ...(windows ? { appLocalRuntimes: true } : { maximumGlibc: "2.35" }) };
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

// Every version source must agree, so installers can never mix release numbers.
export function expectedVersion() {
  const versions = {
    "package.json": JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version,
    "src-tauri/tauri.conf.json": JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8")).version,
    "Cargo.toml": readFileSync(join(root, "Cargo.toml"), "utf8").match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/ms)?.[1],
  };
  const unique = [...new Set(Object.values(versions))];
  assert.ok(unique.length === 1 && unique[0], `Version sources disagree: ${JSON.stringify(versions)}`);
  return unique[0];
}

export function installerVersion(kind, path, windows = process.platform === "win32") {
  const name = path.split(/[\\/]/).at(-1);
  if (kind === "deb") return run("dpkg-deb", ["--field", path, "Version"]).trim();
  if (kind === "rpm") return run("rpm", ["-qp", "--queryformat", "%{VERSION}", path]).trim();
  const pattern = kind === "nsis" ? /^Promptify_(.+)_x64-setup\.exe$/i : /^Promptify_(.+)_amd64\.AppImage$/;
  const version = name.match(pattern)?.[1];
  assert.ok(version, `Cannot read version from ${windows ? "NSIS" : "AppImage"} file name: ${name}`);
  return version;
}

export function verifyInstallers() {
  assert.ok(["linux", "win32"].includes(process.platform), "Only Windows and Linux installers are supported");
  const release = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "release");
  const bundle = join(release, "bundle");
  const staging = join(root, "src-tauri", "binaries");
  const triple = hostTriple();
  const version = expectedVersion();
  // Scratch payloads stay inside the project/build tree and are always removed.
  mkdirSync(bundle, { recursive: true });
  const scratch = mkdtempSync(join(bundle, ".verify-"));
  const report = {
    success: false,
    timestamp: new Date().toISOString(),
    sourceCommit: process.env.SOURCE_COMMIT ?? run("git", ["rev-parse", "HEAD"], { cwd: root }).trim(),
    platform: process.platform,
    hostTriple: triple,
    expectedVersion: version,
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
      const warnings = [];
      const packageVersion = installerVersion(kind, path);
      assert.equal(packageVersion, version, `${kind} installer version differs from source version`);
      if (kind === "nsis") {
        run("7z", ["x", "-y", `-o${destination}`, path]);
      } else if (kind === "deb") {
        assert.equal(run("dpkg-deb", ["--field", path, "Architecture"]).trim(), "amd64", "Debian package is not amd64");
        metadata = excerpt(run("dpkg-deb", ["--info", path]));
        console.log(metadata);
        run("dpkg-deb", ["--extract", path, destination]);
      } else if (kind === "rpm") {
        assert.equal(run("rpm", ["-qp", "--queryformat", "%{ARCH}", path]).trim(), "x86_64", "RPM is not x86_64");
        metadata = excerpt(run("rpm", ["-qp", "--requires", path]));
        console.log(metadata);
        assert.equal(run("rpm", ["-qp", "--queryformat", "%{FILEDIGESTALGO}", path]).trim(), "8", "RPM file digests are not SHA-256");
        const dump = run("rpm", ["-qp", "--dump", path]);
        const archive = join(scratch, `${index}.cpio`);
        const converted = runToFile("rpm2cpio", [path], archive);
        assert.ok(converted.status === 0 || (converted.status === 1 && !converted.stderr.trim()), `rpm2cpio failed (${converted.status ?? converted.signal}):\n${converted.stderr}`);
        runFromFile("cpio", ["--extract", "--make-directories", "--no-absolute-filenames", "--quiet"], archive, destination);
        rmSync(archive);
        const files = verifyRpmExtraction(dump, destination);
        if (converted.status === 1) {
          const warning = `rpm2cpio exited 1 without stderr; accepted only because all ${files} extracted files matched the RPM SHA-256 manifest`;
          warnings.push(warning);
          console.warn(process.env.GITHUB_ACTIONS ? `::warning title=rpm2cpio exit status::${warning}` : `WARNING: ${warning}`);
        }
        metadata += `\nrpm2cpio exit status: ${converted.status}; verified ${files} files against RPM SHA-256 manifest\n`;
      } else {
        chmodSync(path, statSync(path).mode | 0o111);
        run(path, ["--appimage-extract"], { cwd: destination, stdio: ["ignore", "ignore", "pipe"] });
        payload = join(destination, "squashfs-root");
        assert.ok(existsSync(join(payload, "AppRun")), "AppImage is missing AppRun");
      }
      const checks = verifyPayload(payload, staging, undefined, { exactBytes: kind !== "appimage", triple, smokeScratch: join(destination, "smoke") });
      report.installers.push({
        file: path.slice(bundle.length + 1).replaceAll("\\", "/"),
        kind,
        version: packageVersion,
        bytes: statSync(path).size,
        sha256: sha256(path),
        metadata,
        warnings,
        checks,
      });
    }
    report.success = true;
    console.log(`Verified ${installers.length} installer(s); safe to upload.`);
  } catch (error) {
    report.error = excerpt(error?.stack ?? String(error), 32 * 1024);
    throw error;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
    writeFileSync(join(bundle, "verification.json"), `${JSON.stringify(report, null, 2)}\n`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  verifyInstallers();
}
