export type Status = "ok" | "failed" | "not_implemented" | "not_checked" | string;
export type CommandResult<T> = T & { status: Status; message: string };
export type GrokApiBackend = "responses" | "chat_completions" | "messages";
export type GrokModelConfig = { alias: string; model: string; name: string; baseUrl: string; apiBackend: GrokApiBackend; contextWindow: number | null; apiKeyConfigured: boolean };
export type GrokConfigResult = CommandResult<{ grokHome: string; configPath: string; configExists: boolean; cliPath: string | null; cliInstalled: boolean; revision: string; defaultModel: string; modelsBaseUrl: string; models: GrokModelConfig[] }>;
export type GrokModelInput = Omit<GrokModelConfig, "apiKeyConfigured"> & { sourceAlias: string; apiKeyUpdate: string; removeApiKey: boolean };
export type SaveGrokConfigRequest = { revision: string; defaultModel: string; modelsBaseUrl: string; models: GrokModelInput[] };
export type SaveGrokConfigResult = GrokConfigResult & { backupPath: string | null };
export type DraftModel = GrokModelInput & { clientId: string; contextWindowText: string; apiKeyConfigured: boolean };
export type Draft = Omit<SaveGrokConfigRequest, "models"> & { models: DraftModel[] };
let sequence = 0;
const nextId = () => `grok-${Date.now()}-${++sequence}`;

export function grokDraftFromConfig(config: GrokConfigResult): Draft {
  return { revision: config.revision, defaultModel: config.defaultModel, modelsBaseUrl: config.modelsBaseUrl, models: config.models.map((model) => ({ ...model, clientId: nextId(), sourceAlias: model.alias, contextWindowText: model.contextWindow?.toString() ?? "", apiKeyUpdate: "", removeApiKey: false })) };
}

export function grokRequestFromDraft(draft: Draft): SaveGrokConfigRequest {
  return { revision: draft.revision, defaultModel: draft.defaultModel.trim(), modelsBaseUrl: draft.modelsBaseUrl.trim(), models: draft.models.map(({ clientId: _clientId, contextWindowText, apiKeyConfigured: _configured, ...model }) => ({ ...model, alias: model.alias.trim(), model: model.model.trim(), name: model.name.trim(), baseUrl: model.baseUrl.trim(), contextWindow: contextWindowText.trim() ? Number(contextWindowText) : null, apiKeyUpdate: model.apiKeyUpdate.trim() })) };
}

/** 与 Rust 后端一致：别名必填且唯一；模型与 URL 允许为空；窗口须为安全正整数。 */
export function grokDraftValidation(draft: Draft): string {
  const aliases = new Set<string>();
  for (const model of draft.models) {
    const alias = model.alias.trim();
    if (!alias) return "模型别名不能为空。";
    if (aliases.has(alias)) return "模型别名不能重复。";
    aliases.add(alias);
    const value = model.contextWindowText.trim();
    if (value && (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) <= 0)) return `模型「${alias}」的上下文窗口必须是大于 0 的安全整数。`;
    if (model.removeApiKey && model.apiKeyUpdate.trim()) return `模型「${alias}」不能同时替换并移除 API Key。`;
  }
  return "";
}
