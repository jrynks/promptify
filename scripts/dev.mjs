// Starts `tauri dev` after building the llama.cpp worker, with a local build folder and libclang.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const env = { ...process.env };

// Building on a network share is slow and fragile; keep build output on the local disk.
if (!env.CARGO_TARGET_DIR) {
  const base = process.platform === "win32" ? env.LOCALAPPDATA ?? homedir() : join(homedir(), ".cache");
  env.CARGO_TARGET_DIR = join(base, "promptify-target");
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
const run = (cmd, args) => {
  const result = spawnSync(cmd, args, { cwd: root, env, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
};

run("cargo", ["build", "-p", "promptify-llm"]);
run(process.execPath, [join(root, "node_modules", "@tauri-apps", "cli", "tauri.js"), "dev", ...process.argv.slice(2)]);
