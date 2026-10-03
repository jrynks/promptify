// Runs the embedded desktop build without Vite or the source-change watcher.
import { join } from "node:path";
import { env, run } from "./build-env.mjs";

run("cargo", ["build", "-p", "promptify-llm", "-p", "promptify", "--bins", "--features", "promptify/custom-protocol"], { retries: 2 });
run(join(env.CARGO_TARGET_DIR, "debug", process.platform === "win32" ? "promptify.exe" : "promptify"), []);
