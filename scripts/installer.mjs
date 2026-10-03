// Packages the native app with its llama.cpp worker and developer CLI.
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
const bundles = process.platform === "win32" ? "nsis" : process.platform === "linux" ? "deb,rpm,appimage" : null;
if (!bundles) {
  console.error("Installer builds currently support Windows and Linux only.");
  process.exit(1);
}

// Never specialize Whisper to the release builder's CPU (for example AVX-512).
env.GGML_NATIVE = "OFF";
if (process.platform === "linux") env.NO_STRIP = "1";

run("cargo", ["build", "--locked", "--release", "-p", "promptify-llm", "-p", "promptify", "--bin", "promptify-cli", "--bin", "promptify-llm", "--features", "promptify/custom-protocol"], { retries: 2 });

// Tauri installs sidecars next to the app without the target suffix, where `worker_exe()` looks.
// Tauri resolves the path from src-tauri and drops drive letters, so it is staged there.
const staging = join(root, "src-tauri", "binaries");
mkdirSync(staging, { recursive: true });
copyFileSync(join(env.CARGO_TARGET_DIR, "release", `promptify-llm${exe}`), join(staging, `promptify-llm-${triple}${exe}`));
copyFileSync(join(env.CARGO_TARGET_DIR, "release", `promptify-cli${exe}`), join(staging, `promptify-cli-${triple}${exe}`));
const config = { bundle: { externalBin: ["binaries/promptify-llm", "binaries/promptify-cli"], resources: {} } };
if (process.platform === "linux") {
  config.bundle.linux = {
    deb: { depends: ["libc6 (>= 2.35)", "libwebkit2gtk-4.1-0", "libgtk-3-0", "libayatana-appindicator3-1", "libasound2", "libvulkan1", "libgomp1", "libxdo3", "libxtst6", "libxi6", "libxkbcommon0"] },
    rpm: { depends: ["webkit2gtk4.1", "gtk3", "libayatana-appindicator-gtk3", "alsa-lib", "vulkan-loader", "libgomp", "libxdo", "libXtst", "libXi", "libxkbcommon"] },
  };
}

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
run(process.execPath, [tauriCli, "build", "--bundles", bundles, "--ignore-version-mismatches", "--config", JSON.stringify(config), ...process.argv.slice(2), "--", "--locked"]);
console.log(`Installers: ${join(env.CARGO_TARGET_DIR, "release", "bundle")}`);
