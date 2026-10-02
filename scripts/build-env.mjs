// Build environment shared by the dev launcher and the installer build.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
export const env = { ...process.env };

// Building on a network share is slow and fragile; keep build output on the local disk.
// Windows needs a very short path: ggml's Vulkan shader build nests past MAX_PATH otherwise.
if (!env.CARGO_TARGET_DIR) {
  env.CARGO_TARGET_DIR =
    process.platform === "win32" ? join(env.SystemDrive ?? "C:", "\\ptb") : join(homedir(), ".cache", "promptify-target");
}

// llama.cpp's Rust bindings are generated with bindgen, which needs libclang.
if (!env.LIBCLANG_PATH) {
  const candidates = [];
  if (process.platform === "win32") candidates.push("C:\\Program Files\\LLVM\\bin");
  const python = spawnSync(process.platform === "win32" ? "python" : "python3", ["-c", "import clang,os;print(os.path.join(os.path.dirname(clang.__file__),'native'))"], { encoding: "utf8" });
  if (python.status === 0) candidates.push(python.stdout.trim());
  const found = candidates.find((dir) => existsSync(dir));
  if (found) env.LIBCLANG_PATH = found;
  else console.warn("libclang not found: install LLVM or `pip install --user libclang`, or set LIBCLANG_PATH.");
}

console.log(`CARGO_TARGET_DIR=${env.CARGO_TARGET_DIR}`);
if (process.platform !== "darwin" && !env.VULKAN_SDK) {
  console.error("VULKAN_SDK is not set. Install the Vulkan SDK (https://vulkan.lunarg.com) to build the GPU engines.");
  process.exit(1);
}

export const run = (cmd, args, { retries = 0 } = {}) => {
  for (let attempt = 0; ; attempt++) {
    const result = spawnSync(cmd, args, { cwd: root, env, stdio: "inherit" });
    if (result.error) throw result.error;
    if (result.status === 0) return;
    if (attempt >= retries) process.exit(result.status ?? 1);
    console.warn("Build failed; retrying once (ggml's first Vulkan build can race its own install step).");
  }
};

export const tauriCli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
