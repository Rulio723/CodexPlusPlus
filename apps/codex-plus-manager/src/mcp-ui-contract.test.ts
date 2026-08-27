import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const appPath = new URL("./App.tsx", import.meta.url);

test("MCP 表单、预览与导入均通过注册的 Tauri 命令，并在导入后同步 live 配置", async () => {
  const app = await readFile(appPath, "utf8");
  for (const command of ["parse_mcp_entry", "build_mcp_entry", "preview_mcp_servers_json", "import_mcp_servers_json"]) {
    assert.match(app, new RegExp(`call<[^>]+>\\("${command}"`));
  }
  const managerStart = app.indexOf("function RelayContextManager(");
  const managerEnd = app.indexOf("\nfunction SkillManager(", managerStart);
  assert.ok(managerStart >= 0 && managerEnd > managerStart);
  const manager = app.slice(managerStart, managerEnd);
  assert.match(manager, /<McpJsonImporter/);
  assert.match(manager, /syncLiveContextEntries\(next, true, removedEntries\)/);
  assert.match(manager, /syncContextEntries\(next, \[\{ kind: entry\.kind, id: entry\.id \}\]\)/);
  assert.match(app, /request: \{ settings: next, removedEntries \}/);
});

test("供应商默认 name 使用 config 中已有的展示名", async () => {
  const app = await readFile(appPath, "utf8");
  assert.match(app, /resolveProviderName\(next, provider\)/);
  assert.doesNotMatch(app, /setTomlSectionStringKey\(next, section, "name", provider\)/);
});

test("历史 contextSelection 只在规范化时迁移，不参与预览或 live 过滤", async () => {
  const app = await readFile(appPath, "utf8");
  assert.doesNotMatch(app, /function contextEntriesForProfile/);
  assert.doesNotMatch(app, /function filterContextEntriesBySelection/);
  assert.match(app, /const entries = contextEntriesFromSettings\(settings\);/);
  assert.match(app, /contextSelection: _legacyContextSelection/);
});
