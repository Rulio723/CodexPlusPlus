import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const managerCommandsSource = readFileSync(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8");
const managerLibSource = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const grokConfigSource = readFileSync(new URL("./grok-config.tsx", import.meta.url), "utf8");
const remoteSkillsSource = readFileSync(new URL("./remote-skills.tsx", import.meta.url), "utf8");
const coreLibSource = readFileSync(
  new URL("../../../crates/codex-plus-core/src/lib.rs", import.meta.url),
  "utf8",
);
const rendererInjectionSource = readFileSync(
  new URL("../../../assets/inject/renderer-inject.js", import.meta.url),
  "utf8",
);

describe("migrated feature contracts", () => {
  it("registers every Skill command used by the manager UI", () => {
    for (const command of ["list_skills", "import_skill", "set_skill_enabled", "uninstall_skill"]) {
      assert.match(appSource, new RegExp(`\\"${command}\\"`));
      assert.match(managerCommandsSource, new RegExp(`pub fn ${command}\\b`));
      assert.match(managerLibSource, new RegExp(`commands::${command}\\b`));
    }
    assert.match(coreLibSource, /pub mod skill_manager;/);
  });

  it("keeps the remote Skills catalog commands distinct from local SKILL.md commands", () => {
    for (const command of ["refresh_skill_catalog", "list_installed_skills", "install_skill", "update_skill"]) {
      assert.match(appSource, new RegExp(`\\"${command}\\"`));
      assert.match(managerCommandsSource, new RegExp(`pub (?:async )?fn ${command}\\b`));
      assert.match(managerLibSource, new RegExp(`commands::${command}\\b`));
    }
    for (const command of ["set_remote_skill_enabled", "uninstall_remote_skill"]) {
      assert.match(appSource, new RegExp(`\\"${command}\\"`));
      assert.match(managerCommandsSource, new RegExp(`pub (?:async )?fn ${command}\\b`));
      assert.match(managerLibSource, new RegExp(`commands::${command}\\b`));
    }
    assert.match(remoteSkillsSource, /export function RemoteSkillsScreen/);
  });

  it("exposes Grok configuration and pure API no-auth controls", () => {
    assert.match(appSource, /route === "grok"/);
    assert.match(grokConfigSource, /export function GrokConfigScreen/);
    assert.match(appSource, /checked=\{profile\.noAuth\}/);
    assert.match(appSource, /NO_AUTH_PROXY_BEARER_TOKEN/);
  });

  it("maps current Codex rows to persisted session IDs before session actions", () => {
    assert.match(rendererInjectionSource, /function reactConversationIdFromRow\(row\)/);
    assert.match(rendererInjectionSource, /props\.entry\?\.conversationId/);
    assert.match(rendererInjectionSource, /props && props\.entry\?\.conversationId/);
    assert.match(rendererInjectionSource, /!hrefIsTemporary \? reactConversationIdFromRow\(row\) : ""/);
    assert.match(rendererInjectionSource, /postJson\("\/delete", ref\)/);
    assert.match(rendererInjectionSource, /postJson\("\/export-markdown", ref\)/);
    assert.match(rendererInjectionSource, /postJson\("\/move-thread-workspace"/);
  });

  it("keeps Codex thread catalogs synchronized after delete and undo", () => {
    assert.match(rendererInjectionSource, /function notifyCodexThreadState\(method, ref\)/);
    assert.match(rendererInjectionSource, /notifyCodexThreadState\("thread\/deleted", ref\)/);
    assert.match(rendererInjectionSource, /notifyCodexThreadState\("thread\/unarchived", restoredRef\)/);
  });

  it("defines every plugin auto-expand hook used by the renderer scan loop", () => {
    assert.match(rendererInjectionSource, /function schedulePluginAutoExpand\(force = false\)/);
    assert.match(rendererInjectionSource, /function runPluginAutoExpand\(force = false\)/);
    assert.match(rendererInjectionSource, /pluginAutoExpand: "codexAppPluginAutoExpand"/);
    assert.match(rendererInjectionSource, /schedulePluginAutoExpand\(\);/);
  });
});
