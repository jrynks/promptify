import assert from "node:assert/strict";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { findInstallers, smokeCli, verifyInstallers, verifyPayload as verify } from "./verify-installers.mjs";

const windowsTriple = "x86_64-pc-windows-msvc";
const linuxTriple = "x86_64-unknown-linux-gnu";
const verifyPayload = (payload, staging, windows, options = {}) =>
  verify(payload, staging, windows, { triple: windows ? windowsTriple : linuxTriple, ...options });

function fixture(t) {
  const root = mkdtempSync(join(process.cwd(), ".verify-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const payload = join(root, "payload");
  const release = join(root, "staging");
  mkdirSync(payload);
  mkdirSync(release);
  const pe = Buffer.alloc(128);
  pe.write("MZ");
  pe.writeUInt32LE(64, 0x3c);
  pe.write("PE\0\0", 64);
  pe.writeUInt16LE(0x8664, 68);
  writeFileSync(join(payload, "promptify.exe"), pe);
  for (const name of ["promptify-llm", "promptify-cli"]) {
    writeFileSync(join(payload, `${name}.exe`), pe);
    writeFileSync(join(release, `${name}-${windowsTriple}.exe`), pe);
  }
  for (const name of ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll", "vcomp140.dll"]) {
    writeFileSync(join(payload, name), `runtime ${name}`);
    writeFileSync(join(release, name), `runtime ${name}`);
  }
  return { root, payload, release };
}

test("accepts an x64 app with the unchanged worker, CLI and local runtimes", (t) => {
  const { payload, release } = fixture(t);
  verifyPayload(payload, release, true);
});

test("rejects a missing CLI or target-suffixed/misplaced worker", (t) => {
  const { payload, release } = fixture(t);
  rmSync(join(payload, "promptify-cli.exe"));
  assert.throws(() => verifyPayload(payload, release, true), /Missing executable/);
  writeFileSync(join(payload, "promptify-cli.exe"), Buffer.alloc(128));
  rmSync(join(payload, "promptify-llm.exe"));
  writeFileSync(join(payload, "promptify-llm-x86_64-pc-windows-msvc.exe"), "worker");
  assert.throws(() => verifyPayload(payload, release, true), /Missing executable/);
});

test("rejects architecture, changed binaries and absent runtime DLLs", (t) => {
  const { payload, release } = fixture(t);
  const wrong = Buffer.alloc(128);
  wrong.write("MZ");
  wrong.writeUInt32LE(64, 0x3c);
  wrong.write("PE\0\0", 64);
  wrong.writeUInt16LE(0x14c, 68);
  writeFileSync(join(payload, "promptify.exe"), wrong);
  assert.throws(() => verifyPayload(payload, release, true), /Not a Windows x64/);
  wrong.writeUInt16LE(0x8664, 68);
  writeFileSync(join(payload, "promptify.exe"), wrong);
  wrong[100] = 1;
  writeFileSync(join(payload, "promptify-cli.exe"), wrong);
  assert.throws(() => verifyPayload(payload, release, true), /differs from staged sidecar/);
  assert.equal(verifyPayload(payload, release, true, { exactBytes: false }).matchesStagedSidecars, false);
  writeFileSync(join(release, `promptify-cli-${windowsTriple}.exe`), wrong);
  writeFileSync(join(payload, "vcomp140.dll"), "changed runtime");
  assert.throws(() => verifyPayload(payload, release, true), /differs from staged sidecar/);
  rmSync(join(payload, "vcomp140.dll"));
  assert.throws(() => verifyPayload(payload, release, true), /Missing app-local runtime/);
});

test("requires every requested package format, not merely some uploadable file", (t) => {
  const { root } = fixture(t);
  mkdirSync(join(root, "nsis"));
  writeFileSync(join(root, "nsis", "Promptify_1.1.0_x64-setup.exe"), "installer");
  assert.equal(findInstallers(root, true).length, 1);
  mkdirSync(join(root, "deb"));
  writeFileSync(join(root, "deb", "promptify.deb"), "installer");
  assert.throws(() => findInstallers(root, false), /Missing rpm/);
  mkdirSync(join(root, "rpm"));
  writeFileSync(join(root, "rpm", "promptify.rpm"), "installer");
  mkdirSync(join(root, "appimage"));
  assert.throws(() => findInstallers(root, false), /No appimage installers/);
  writeFileSync(join(root, "appimage", "Promptify.AppImage"), "installer");
  assert.equal(findInstallers(root, false).length, 3);
});

test("Linux payload verifies ELF architecture, glibc versions and executable permissions", { skip: process.platform !== "linux" }, (t) => {
  const { payload, release } = fixture(t);
  copyFileSync("/usr/bin/true", join(payload, "promptify"));
  for (const name of ["promptify-llm", "promptify-cli"]) {
    copyFileSync("/usr/bin/true", join(payload, name));
    copyFileSync("/usr/bin/true", join(release, `${name}-${linuxTriple}`));
  }
  verifyPayload(payload, release, false);
  chmodSync(join(payload, "promptify-llm"), 0o644);
  assert.throws(() => verifyPayload(payload, release, false), /Executable permission is missing/);
});

test("failed inspection persists its report and removes extracted payloads", (t) => {
  const { root } = fixture(t);
  const previous = { CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR, SOURCE_COMMIT: process.env.SOURCE_COMMIT };
  process.env.CARGO_TARGET_DIR = root;
  process.env.SOURCE_COMMIT = "0123456789abcdef0123456789abcdef01234567";
  t.after(() => {
    for (const [name, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  });
  assert.throws(() => verifyInstallers(), /Missing .* installer directory/);
  const bundle = join(root, "release", "bundle");
  const report = JSON.parse(readFileSync(join(bundle, "verification.json"), "utf8"));
  assert.equal(report.success, false);
  assert.equal(report.sourceCommit, process.env.SOURCE_COMMIT);
  assert.match(report.error, /Missing .* installer directory/);
  assert.equal(readdirSync(bundle).filter((name) => name.startsWith(".verify-")).length, 0);
});

test("packaged CLI smoke uses isolated data and requires every bundled model", { skip: process.platform !== "linux" }, (t) => {
  const { root } = fixture(t);
  const manifest = JSON.parse(readFileSync("crates/promptify-core/models/manifest.json", "utf8"));
  const lines = manifest.models.map((model) => `${model.id} Llm/Small installed=false selected=false  test`).join("\\n");
  const cli = join(root, "fake-cli");
  writeFileSync(cli, `#!/bin/sh\ntest "$1" = models || exit 2\ntest "$PROMPTIFY_DATA_DIR" = "${join(root, "smoke", "cli-data")}" || exit 3\nprintf '${lines}\\nmodels folder: %s\\n' "$PROMPTIFY_MODELS_DIR"\n`);
  chmodSync(cli, 0o755);
  assert.equal(smokeCli(cli, join(root, "smoke")).models, manifest.models.length);
  writeFileSync(cli, "#!/bin/sh\necho 'models folder: elsewhere'\n");
  assert.throws(() => smokeCli(cli, join(root, "smoke")), /did not list model/);
});
