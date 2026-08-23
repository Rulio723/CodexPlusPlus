import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFile } from "node:fs/promises";

const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);
const appPath = new URL("./App.tsx", import.meta.url);

describe("v1.2.51 merge layout regressions", () => {
  it("keeps the Codex++ page modal free of removed ads runtime references", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const start = renderer.indexOf("  function openCodexPlusModal(options = {})");
    const end = renderer.indexOf("\n  function openCodexPlusPage()", start);
    assert.ok(start >= 0 && end > start);
    const modal = renderer.slice(start, end);

    assert.doesNotMatch(renderer, /\bcodexPlusAdsLoaded\b|\bfetchCodexPlusAds\b/);
    assert.match(modal, /document\.body\.appendChild\(overlay\)/);
    assert.match(modal, /selectCodexPlusTab\("home"\)/);
    assert.match(modal, /renderCodexPlusMenu\(\)/);
  });

  it("renders provider repair controls exactly once in SessionsScreen", async () => {
    const app = await readFile(appPath, "utf8");
    const start = app.indexOf("function SessionsScreen(");
    const end = app.indexOf("\nfunction MaintenanceScreen(", start);
    assert.ok(start >= 0 && end > start);
    const sessions = app.slice(start, end);
    const switches = sessions.match(/checked=\{form\.providerSyncEnabled\}/g) ?? [];

    assert.equal(switches.length, 1);
    assert.match(sessions, /className="session-repair-tools"/);
    assert.match(sessions, /actions\.saveSettings\(\)/);
    assert.match(sessions, /actions\.importSessionUrl\(\)/);
    assert.doesNotMatch(sessions, /保存自动修复设置/);
  });
});
