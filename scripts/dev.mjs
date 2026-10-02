// Starts `tauri dev` after building the llama.cpp worker, with a local build folder and libclang.
import { run, tauriCli } from "./build-env.mjs";

run("cargo", ["build", "-p", "promptify-llm", "-p", "promptify", "--bins"], { retries: 2 });
run(process.execPath, [tauriCli, "dev", ...process.argv.slice(2)]);
