import assert from "node:assert/strict";
import test from "node:test";

import {
  grokDraftFromConfig,
  grokDraftValidation,
  grokRequestFromDraft,
  type GrokConfigResult,
} from "./grok-config-logic.ts";

const config: GrokConfigResult = {
  status: "ok",
  message: "ok",
  grokHome: "C:/grok",
  configPath: "C:/grok/config.toml",
  configExists: true,
  cliPath: null,
  cliInstalled: false,
  revision: "r1",
  defaultModel: "primary",
  modelsBaseUrl: "",
  models: [{ alias: "primary", model: "", name: "", baseUrl: "", apiBackend: "responses", contextWindow: null, apiKeyConfigured: false }],
};

test("Grok 草稿允许后端允许为空的模型和 Base URL", () => {
  const draft = grokDraftFromConfig(config);
  assert.equal(grokDraftValidation(draft), "");
});

test("Grok 请求在重命名时保留 sourceAlias 并修剪输入", () => {
  const draft = grokDraftFromConfig(config);
  draft.models[0].alias = " renamed ";
  draft.models[0].model = "  ";
  const request = grokRequestFromDraft(draft);
  assert.equal(request.models[0].sourceAlias, "primary");
  assert.equal(request.models[0].alias, "renamed");
  assert.equal(request.models[0].model, "");
});

test("Grok 上下文窗口拒绝超过 JavaScript 安全整数的值", () => {
  const draft = grokDraftFromConfig(config);
  draft.models[0].contextWindowText = "9007199254740992";
  assert.match(grokDraftValidation(draft), /安全整数/);
});

test("Grok 不允许同时替换和删除同一 API Key", () => {
  const draft = grokDraftFromConfig(config);
  draft.models[0].apiKeyUpdate = "new-key";
  draft.models[0].removeApiKey = true;
  assert.match(grokDraftValidation(draft), /同时替换并移除/);
});
