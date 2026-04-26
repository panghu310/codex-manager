import { execFileSync, execSync } from "child_process";
import fs from "fs";
import path from "path";

const root = path.resolve(process.cwd());
const binariesDir = path.join(root, "src-tauri", "binaries");
const targetFlagIndex = process.argv.indexOf("--target");
const explicitTarget =
  targetFlagIndex >= 0 ? process.argv[targetFlagIndex + 1]?.trim() : "";

if (targetFlagIndex >= 0 && !explicitTarget) {
  console.error("--target 需要跟一个 Rust target triple");
  process.exit(1);
}

let targetTriple = explicitTarget;
if (!targetTriple) {
  const rustInfo = execSync("rustc -vV", { encoding: "utf8" });
  const targetMatch = /host: (\S+)/.exec(rustInfo);
  if (!targetMatch) {
    console.error("无法获取 Rust target triple");
    process.exit(1);
  }
  targetTriple = targetMatch[1];
}

const profile = process.argv.includes("--release") ? "release" : "debug";
const cargoArgs = ["build", "--bin", "telegram-codex-bot"];
if (process.argv.includes("--release")) {
  cargoArgs.push("--release");
}
if (explicitTarget) {
  cargoArgs.push("--target", targetTriple);
}

fs.mkdirSync(binariesDir, { recursive: true });
const dest = path.join(binariesDir, `telegram-codex-bot-${targetTriple}`);

// 先创建占位文件，避免 Tauri build script 检查失败
if (!fs.existsSync(dest)) {
  fs.writeFileSync(dest, "");
}

console.log(`构建 telegram-codex-bot (${profile})...`);
execFileSync("cargo", cargoArgs, {
  cwd: path.join(root, "src-tauri"),
  stdio: "inherit"
});

const targetSegments = explicitTarget ? [targetTriple, profile] : [profile];
const src = path.join(root, "src-tauri", "target", ...targetSegments, "telegram-codex-bot");
if (!fs.existsSync(src)) {
  console.error(`构建产物不存在: ${src}`);
  process.exit(1);
}

fs.copyFileSync(src, dest);
fs.chmodSync(dest, 0o755);

console.log(`已复制 sidecar: ${dest}`);
