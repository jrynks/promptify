// Builds a Windows installer (NSIS) with the llama.cpp worker bundled next to the app.
import { copyFileSync, existsSync, mkdirSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

import { env, root, run, tauriCli } from "./build-env.mjs";

const triple = spawnSync("rustc", ["-vV"], { encoding: "utf8" }).stdout.match(/^host: (\S+)$/m)?.[1];
if (!triple) {
  console.error("could not read the Rust host target from `rustc -vV`");
  process.exit(1);
}
const exe = process.platform === "win32" ? ".exe" : "";

run("cargo", ["build", "--release", "-p", "promptify-llm"], { retries: 2 });

// Tauri installs sidecars next to the app without the target suffix, where `worker_exe()` looks.
// Tauri resolves the path from src-tauri and drops drive letters, so it is staged there.
const staging = join(root, "src-tauri", "binaries");
mkdirSync(staging, { recursive: true });
copyFileSync(join(env.CARGO_TARGET_DIR, "release", `promptify-llm${exe}`), join(staging, `promptify-llm-${triple}${exe}`));
const config = { bundle: { externalBin: ["binaries/promptify-llm"], resources: {} } };

// The app and worker need the Visual C++ runtime and OpenMP, which a fresh Windows may lack.
// Microsoft allows installing these redistributable DLLs next to the app.
if (process.platform === "win32") {
  const vswhere = join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe");
  const vs = spawnSync(vswhere, ["-latest", "-products", "*", "-property", "installationPath"], { encoding: "utf8" }).stdout?.trim();
  const redistRoot = vs && join(vs, "VC", "Redist", "MSVC");
  const version = redistRoot && existsSync(redistRoot) && readdirSync(redistRoot).filter((d) => /^\d+\.\d+\.\d+$/.test(d)).sort().at(-1);
  if (!version) {
    console.error("Visual C++ redistributable DLLs not found (Visual Studio Build Tools > VC\\Redist\\MSVC).");
    process.exit(1);
  }
  const x64 = join(redistRoot, version, "x64");
  const dlls = { "Microsoft.VC143.CRT": ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"], "Microsoft.VC143.OpenMP": ["vcomp140.dll"] };
  for (const [folder, names] of Object.entries(dlls)) {
    for (const name of names) {
      copyFileSync(join(x64, folder, name), join(staging, name));
      config.bundle.resources[`binaries/${name}`] = name;
    }
  }
  console.log(`Bundling Visual C++ runtime ${version}`);
}

// The tauri crate is 2.12 but npm has no @tauri-apps/api 2.12 yet; dev builds run this same pair.
run(process.execPath, [tauriCli, "build", "--bundles", "nsis", "--ignore-version-mismatches", "--config", JSON.stringify(config), ...process.argv.slice(2)]);
console.log(`Installer: ${join(env.CARGO_TARGET_DIR, "release", "bundle", "nsis")}`);
