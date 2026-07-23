import { spawn } from "node:child_process";
import path from "node:path";

const run = (command, args, options = {}) =>
  new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: process.cwd(),
      env: process.env,
      stdio: "inherit",
      ...options,
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${command} 运行失败: code=${code ?? "-"} signal=${signal ?? "-"}`));
    });
  });

if (process.platform !== "win32") {
  throw new Error("tauri:release:windows 只能在 Windows 环境运行");
}

const pnpm = "pnpm.cmd";
await run(pnpm, ["run", "install-bins"], {
  env: { ...process.env, BIN_DOWNLOAD: process.env.BIN_DOWNLOAD || "1" },
});
await run(pnpm, [
  "exec",
  "tauri",
  "build",
  "--config",
  "src-tauri/tauri.windows.conf.json",
  "--no-bundle",
]);
await run(path.resolve("src-tauri/target/release/bili-clip-flow.exe"), []);
