import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const nativeCdpTest = process.platform === "win32" ? test : test.skip;

nativeCdpTest("工具与插件与 Skills 导航在真实 WebView 渲染中保持可挂载", { timeout: 60_000 }, async () => {
  const harness = new URL("./ui-navigation-render.mjs", import.meta.url);
  const { stdout, stderr } = await execFileAsync(process.execPath, [fileURLToPath(harness)], {
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    windowsHide: true,
  });

  assert.match(stdout, /context: Codex 工具与插件/);
  assert.match(stdout, /skills: 技能列表/);
  assert.equal(stderr, "");
  process.stdout.write(stdout);
});
