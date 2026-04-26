import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { execSync } from "node:child_process";

const root = path.resolve(import.meta.dirname, "..");

function read(relativePath) {
  return fs.readFileSync(path.join(root, relativePath), "utf8");
}

function readJson(relativePath) {
  return JSON.parse(read(relativePath));
}

function capture(pattern, text, label) {
  const match = text.match(pattern);
  assert.ok(match, `未找到 ${label}`);
  return match[1];
}

function parseVersion(version) {
  const match = String(version).trim().match(/^(\d+)\.(\d+)\.(\d+)$/);
  assert.ok(match, `无法解析版本号：${version}`);
  return match.slice(1).map((part) => Number(part));
}

function compareVersion(left, right) {
  const [la, lb, lc] = parseVersion(left);
  const [ra, rb, rc] = parseVersion(right);
  if (la !== ra) return Math.sign(la - ra);
  if (lb !== rb) return Math.sign(lb - rb);
  return Math.sign(lc - rc);
}

test("发布版本号在 package、Tauri 和 Cargo 之间保持一致", () => {
  const packageVersion = readJson("package.json").version;
  const tauriVersion = readJson("src-tauri/tauri.conf.json").version;
  const cargoVersion = capture(
    /^version = "([^"]+)"/m,
    read("src-tauri/Cargo.toml"),
    "Cargo.toml version"
  );

  assert.equal(packageVersion, tauriVersion);
  assert.equal(packageVersion, cargoVersion);
});

test("发布版本号与最新 git tag 关系正确", () => {
  const packageVersion = readJson("package.json").version;
  const latestTag = execSync("git tag --list 'v*' --sort=-version:refname | head -n 1", {
    cwd: root,
    encoding: "utf8"
  }).trim();
  const changedVersionFiles = execSync(
    "git diff --name-only HEAD -- package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json",
    {
      cwd: root,
      encoding: "utf8"
    }
  )
    .trim()
    .split("\n")
    .filter(Boolean);

  assert.ok(latestTag, "仓库不存在 v* tag，无法校验发布版本");
  const latestVersion = latestTag.replace(/^v/, "");
  const relation = compareVersion(packageVersion, latestVersion);

  assert.ok(relation >= 0, `当前版本 ${packageVersion} 不能低于最新 tag ${latestVersion}`);
  if (changedVersionFiles.length > 0) {
    assert.ok(
      relation > 0,
      `版本文件已修改，当前版本 ${packageVersion} 必须高于最新 tag ${latestVersion}`
    );
  }
});

test("README 不再声明 launchd plist 作为运行前提", () => {
  const readme = read("README.md");

  assert.equal(
    readme.includes("launchd label"),
    false,
    "README 仍然在描述已经废弃的 launchd 方案"
  );
  assert.equal(
    readme.includes("com.local.telegram-codex-bot"),
    false,
    "README 仍然暴露旧的 launchd label"
  );
});

test("Release workflow 在 src-tauri 目录构建 sidecar，并使用矩阵 target 路径复制产物", () => {
  const workflow = read(".github/workflows/release.yml");

  assert.match(
    workflow,
    /working-directory:\s*src-tauri/,
    "release workflow 没有在 src-tauri 目录执行 cargo build"
  );
  assert.match(
    workflow,
    /cp "target\/\$\{\{\s*matrix\.target\s*\}\}\/release\/telegram-codex-bot"/,
    "release workflow 没有从矩阵 target 对应目录复制 sidecar"
  );
});
