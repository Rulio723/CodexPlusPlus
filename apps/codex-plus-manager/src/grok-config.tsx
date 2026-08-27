import { Bot, Plus, RefreshCw, Save, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { t } from "@/i18n";

type Status = "ok" | "failed" | "not_implemented" | "not_checked" | string;
type CommandResult<T> = T & { status: Status; message: string };
type GrokApiBackend = "responses" | "chat_completions" | "messages";

type GrokModelConfig = {
  alias: string;
  model: string;
  name: string;
  baseUrl: string;
  apiBackend: GrokApiBackend;
  contextWindow: number | null;
  apiKeyConfigured: boolean;
};

export type GrokConfigResult = CommandResult<{
  grokHome: string;
  configPath: string;
  configExists: boolean;
  cliPath: string | null;
  cliInstalled: boolean;
  revision: string;
  defaultModel: string;
  modelsBaseUrl: string;
  models: GrokModelConfig[];
}>;

type GrokModelInput = Omit<GrokModelConfig, "apiKeyConfigured"> & {
  sourceAlias: string;
  apiKeyUpdate: string;
  removeApiKey: boolean;
};

export type SaveGrokConfigRequest = {
  revision: string;
  defaultModel: string;
  modelsBaseUrl: string;
  models: GrokModelInput[];
};

export type SaveGrokConfigResult = GrokConfigResult & { backupPath: string | null };

type DraftModel = GrokModelInput & { clientId: string; contextWindowText: string; apiKeyConfigured: boolean };
type Draft = Omit<SaveGrokConfigRequest, "models"> & { models: DraftModel[] };
let sequence = 0;
const nextId = () => `grok-${Date.now()}-${++sequence}`;

function draftFromConfig(config: GrokConfigResult): Draft {
  return {
    revision: config.revision,
    defaultModel: config.defaultModel,
    modelsBaseUrl: config.modelsBaseUrl,
    models: config.models.map((model) => ({
      ...model,
      clientId: nextId(),
      sourceAlias: model.alias,
      contextWindowText: model.contextWindow?.toString() ?? "",
      apiKeyUpdate: "",
      removeApiKey: false,
    })),
  };
}

function requestFromDraft(draft: Draft): SaveGrokConfigRequest {
  return {
    revision: draft.revision,
    defaultModel: draft.defaultModel.trim(),
    modelsBaseUrl: draft.modelsBaseUrl.trim(),
    models: draft.models.map(({ clientId: _clientId, contextWindowText, apiKeyConfigured: _configured, ...model }) => ({
      ...model,
      alias: model.alias.trim(),
      model: model.model.trim(),
      name: model.name.trim(),
      baseUrl: model.baseUrl.trim(),
      contextWindow: contextWindowText.trim() ? Number(contextWindowText) : null,
      apiKeyUpdate: model.apiKeyUpdate.trim(),
    })),
  };
}

function validation(draft: Draft): string {
  const aliases = new Set<string>();
  for (const model of draft.models) {
    if (!model.alias.trim() || !model.model.trim() || !model.baseUrl.trim()) return t("每个 Grok 模型都需要别名、模型名和 Base URL。");
    if (aliases.has(model.alias.trim())) return t("Grok 模型别名不能重复。");
    aliases.add(model.alias.trim());
    if (model.contextWindowText.trim() && (!/^\d+$/.test(model.contextWindowText.trim()) || Number(model.contextWindowText) <= 0)) {
      return t("上下文窗口必须是正整数或留空。");
    }
  }
  return "";
}

export function GrokConfigScreen({ config, onRefresh, onSave }: {
  config: GrokConfigResult | null;
  onRefresh: () => Promise<GrokConfigResult | null>;
  onSave: (request: SaveGrokConfigRequest) => Promise<SaveGrokConfigResult | null>;
}) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    if (!config) return;
    const next = draftFromConfig(config);
    setDraft(next);
    setSelectedId(next.models[0]?.clientId ?? null);
  }, [config]);
  const selected = draft?.models.find((model) => model.clientId === selectedId) ?? null;
  const error = draft ? validation(draft) : "";
  const update = (patch: Partial<DraftModel>) => setDraft((current) => current && selectedId ? {
    ...current,
    models: current.models.map((model) => model.clientId === selectedId ? { ...model, ...patch } : model),
  } : current);
  const add = () => setDraft((current) => {
    if (!current) return current;
    const model: DraftModel = {
      clientId: nextId(), sourceAlias: "", alias: `grok-model-${current.models.length + 1}`, model: "", name: "",
      baseUrl: current.modelsBaseUrl, apiBackend: "responses", contextWindow: null, contextWindowText: "",
      apiKeyConfigured: false, apiKeyUpdate: "", removeApiKey: false,
    };
    setSelectedId(model.clientId);
    return { ...current, models: [...current.models, model] };
  });
  const remove = () => setDraft((current) => {
    if (!current || !selectedId) return current;
    const models = current.models.filter((model) => model.clientId !== selectedId);
    setSelectedId(models[0]?.clientId ?? null);
    return { ...current, models };
  });
  const save = async () => {
    if (!draft || error) return;
    setSaving(true);
    try { await onSave(requestFromDraft(draft)); } finally { setSaving(false); }
  };

  if (!config || !draft) return <section className="panel grok-page"><p>{t("正在读取本机 Grok 配置…")}</p></section>;
  return <div className="grok-page">
    <section className="panel grok-overview-panel">
      <div className="panel-head"><h2>{t("Grok 配置")}</h2><p>{config.configPath}</p></div>
      <div className="grok-overview-content">
        <div className="grok-status-strip"><div><span>Grok CLI</span><strong data-status={config.cliInstalled ? "ok" : "missing"}>{config.cliInstalled ? t("已安装") : t("未安装")}</strong></div><div><span>{t("配置目录")}</span><code>{config.grokHome}</code></div></div>
        <div className="grok-global-fields">
          <Label className="grok-form-row"><span>{t("默认模型")}</span><Input value={draft.defaultModel} onChange={(event) => setDraft({ ...draft, defaultModel: event.currentTarget.value })} /></Label>
          <Label className="grok-form-row"><span>Models Base URL</span><Input value={draft.modelsBaseUrl} onChange={(event) => setDraft({ ...draft, modelsBaseUrl: event.currentTarget.value })} /></Label>
        </div>
      </div>
    </section>
    <div className="grok-manager-grid">
      <section className="panel grok-model-list-panel"><div className="grok-panel-title"><strong>{t("模型")}</strong><Button size="sm" onClick={add}><Plus className="h-4 w-4" />{t("新增")}</Button></div><div className="grok-model-list">{draft.models.map((model) => <button className={`grok-model-item ${model.clientId === selectedId ? "active" : ""}`} key={model.clientId} type="button" onClick={() => setSelectedId(model.clientId)}><span className="grok-model-mark"><Bot className="h-4 w-4" /></span><span className="grok-model-copy"><strong>{model.alias || t("未命名")}</strong><small>{model.model || "-"}</small></span></button>)}</div></section>
      <section className="panel grok-editor-panel">{selected ? <div className="grok-editor-fields">
        <Label className="grok-form-row"><span>{t("别名")}</span><Input value={selected.alias} onChange={(event) => update({ alias: event.currentTarget.value })} /></Label>
        <Label className="grok-form-row"><span>{t("模型")}</span><Input value={selected.model} onChange={(event) => update({ model: event.currentTarget.value })} /></Label>
        <Label className="grok-form-row"><span>{t("名称")}</span><Input value={selected.name} onChange={(event) => update({ name: event.currentTarget.value })} /></Label>
        <Label className="grok-form-row"><span>Base URL</span><Input value={selected.baseUrl} onChange={(event) => update({ baseUrl: event.currentTarget.value })} /></Label>
        <Label className="grok-form-row"><span>{t("上游协议")}</span><select className="field-select" value={selected.apiBackend} onChange={(event) => update({ apiBackend: event.currentTarget.value as GrokApiBackend })}><option value="responses">Responses API</option><option value="chat_completions">Chat Completions</option><option value="messages">Messages API</option></select></Label>
        <Label className="grok-form-row"><span>{t("上下文窗口")}</span><Input inputMode="numeric" value={selected.contextWindowText} onChange={(event) => update({ contextWindowText: event.currentTarget.value.replace(/[^\d]/g, "") })} /></Label>
        <Label className="grok-form-row"><span>API Key</span><Input disabled={selected.removeApiKey} type="password" value={selected.apiKeyUpdate} placeholder={selected.apiKeyConfigured ? t("留空保持当前 Key") : t("输入 API Key")} onChange={(event) => update({ apiKeyUpdate: event.currentTarget.value })} /></Label>
        {selected.apiKeyConfigured ? <label className="inline-check"><input checked={selected.removeApiKey} type="checkbox" onChange={(event) => update({ removeApiKey: event.currentTarget.checked, apiKeyUpdate: event.currentTarget.checked ? "" : selected.apiKeyUpdate })} /><span>{t("移除当前 API Key")}</span></label> : null}
        <Button variant="ghost" onClick={remove}><Trash2 className="h-4 w-4" />{t("删除模型")}</Button>
      </div> : <div className="empty">{t("选择或新增一个 Grok 模型")}</div>}</section>
    </div>
    <div className="settings-save-bar grok-save-bar"><span className={error ? "is-error" : ""}>{error || t("Grok 配置有未保存修改")}</span><div className="toolbar"><Button variant="secondary" disabled={saving} onClick={() => void onRefresh()}><RefreshCw className="h-4 w-4" />{t("刷新")}</Button><Button disabled={saving || Boolean(error)} onClick={() => void save()}><Save className="h-4 w-4" />{saving ? t("保存中") : t("保存配置")}</Button></div></div>
  </div>;
}
