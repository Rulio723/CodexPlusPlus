import { Bot, Plus, RefreshCw, RotateCcw, Save, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { t, tf } from "@/i18n";
import {
  grokDraftFromConfig,
  grokDraftValidation,
  grokRequestFromDraft,
  type Draft,
  type DraftModel,
  type GrokApiBackend,
  type GrokConfigResult,
  type SaveGrokConfigRequest,
  type SaveGrokConfigResult,
} from "./grok-config-logic";

export type { GrokConfigResult, SaveGrokConfigRequest, SaveGrokConfigResult } from "./grok-config-logic";
let screenSequence = 0;
const nextScreenId = () => `grok-${Date.now()}-${++screenSequence}`;
export function GrokConfigScreen({ config, onRefresh, onSave }: { config: GrokConfigResult | null; onRefresh: () => Promise<GrokConfigResult | null>; onSave: (request: SaveGrokConfigRequest) => Promise<SaveGrokConfigResult | null> }) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => { if (!config) return; const next = grokDraftFromConfig(config); setDraft(next); setSelectedId((current) => next.models.some((model) => model.clientId === current) ? current : next.models[0]?.clientId ?? null); }, [config]);
  const savedRequest = useMemo(() => config ? grokRequestFromDraft(grokDraftFromConfig(config)) : null, [config]);
  const currentRequest = useMemo(() => draft ? grokRequestFromDraft(draft) : null, [draft]);
  const dirty = Boolean(savedRequest && currentRequest && JSON.stringify(savedRequest) !== JSON.stringify(currentRequest));
  const selected = draft?.models.find((model) => model.clientId === selectedId) ?? null;
  const error = draft ? grokDraftValidation(draft) : "";
  const update = (patch: Partial<DraftModel>) => setDraft((current) => {
    if (!current || !selectedId) return current;
    const selectedModel = current.models.find((model) => model.clientId === selectedId);
    const nextDefault = selectedModel && patch.alias !== undefined && current.defaultModel === selectedModel.alias ? patch.alias : current.defaultModel;
    return { ...current, defaultModel: nextDefault, models: current.models.map((model) => model.clientId === selectedId ? { ...model, ...patch } : model) };
  });
  const add = () => setDraft((current) => {
    if (!current) return current;
    const aliases = new Set(current.models.map((model) => model.alias)); let index = current.models.length + 1; while (aliases.has(`grok-model-${index}`)) index += 1;
    const model: DraftModel = { clientId: nextScreenId(), sourceAlias: "", alias: `grok-model-${index}`, model: "", name: "", baseUrl: "", apiBackend: "responses", contextWindow: null, contextWindowText: "", apiKeyConfigured: false, apiKeyUpdate: "", removeApiKey: false };
    setSelectedId(model.clientId); return { ...current, defaultModel: current.defaultModel || model.alias, models: [...current.models, model] };
  });
  const remove = () => {
    if (!draft || !selected) return;
    if (!window.confirm(tf("删除 Grok 模型「{0}」？", [selected.alias || t("未命名")]))) return;
    const models = draft.models.filter((model) => model.clientId !== selected.clientId);
    setDraft({ ...draft, defaultModel: draft.defaultModel === selected.alias ? models[0]?.alias ?? "" : draft.defaultModel, models }); setSelectedId(models[0]?.clientId ?? null);
  };
  const discard = () => { if (!config) return; if (dirty && !window.confirm(t("放弃未保存的 Grok 配置修改？"))) return; const next = grokDraftFromConfig(config); setDraft(next); setSelectedId(next.models[0]?.clientId ?? null); };
  const refresh = async () => { if (dirty && !window.confirm(t("重新读取会放弃未保存的 Grok 配置修改，继续吗？"))) return; await onRefresh(); };
  const save = async () => { if (!draft || error || !dirty) return; setSaving(true); try { await onSave(grokRequestFromDraft(draft)); } finally { setSaving(false); } };
  if (!config || !draft) return <section className="panel grok-page"><p>{t("正在读取本机 Grok 配置…")}</p></section>;
  return <div className="grok-page">
    <section className="panel grok-overview-panel"><div className="panel-head"><h2>{t("Grok 配置")}</h2><p>{config.configPath}</p></div><div className="grok-overview-content"><div className="grok-status-strip"><div><span>Grok CLI</span><strong data-status={config.cliInstalled ? "ok" : "missing"}>{config.cliInstalled ? t("已安装") : t("未安装")}</strong><code>{config.cliPath || t("未找到可执行文件")}</code></div><div><span>config.toml</span><strong data-status={config.configExists ? "ok" : "missing"}>{config.configExists ? t("已存在") : t("保存时创建")}</strong><code>{config.grokHome}</code></div></div><div className="grok-global-fields"><Label className="grok-form-row"><span>{t("默认模型")}</span><Input list="grok-model-aliases" value={draft.defaultModel} onChange={(event) => setDraft({ ...draft, defaultModel: event.currentTarget.value })} /></Label><datalist id="grok-model-aliases">{draft.models.map((model) => <option key={model.clientId} value={model.alias} />)}</datalist><Label className="grok-form-row"><span>Models Base URL</span><Input value={draft.modelsBaseUrl} onChange={(event) => setDraft({ ...draft, modelsBaseUrl: event.currentTarget.value })} /></Label></div></div></section>
    <div className="grok-manager-grid"><section className="panel grok-model-list-panel"><div className="grok-panel-title"><strong>{t("模型")}</strong><Button size="sm" onClick={add}><Plus className="h-4 w-4" />{t("新增")}</Button></div><div className="grok-model-list">{draft.models.map((model) => <button className={`grok-model-item ${model.clientId === selectedId ? "active" : ""}`} key={model.clientId} type="button" onClick={() => setSelectedId(model.clientId)}><span className="grok-model-mark"><Bot className="h-4 w-4" /></span><span className="grok-model-copy"><strong>{model.alias || t("未命名")}</strong><small>{model.model || "-"}</small></span></button>)}</div></section><section className="panel grok-editor-panel">{selected ? <div className="grok-editor-fields"><Label className="grok-form-row"><span>{t("别名")}</span><Input value={selected.alias} onChange={(event) => update({ alias: event.currentTarget.value })} /></Label><Label className="grok-form-row"><span>{t("模型")}</span><Input value={selected.model} onChange={(event) => update({ model: event.currentTarget.value })} /></Label><Label className="grok-form-row"><span>{t("名称")}</span><Input value={selected.name} onChange={(event) => update({ name: event.currentTarget.value })} /></Label><Label className="grok-form-row"><span>Base URL</span><Input value={selected.baseUrl} onChange={(event) => update({ baseUrl: event.currentTarget.value })} /></Label><Label className="grok-form-row"><span>{t("上游协议")}</span><select className="field-select" value={selected.apiBackend} onChange={(event) => update({ apiBackend: event.currentTarget.value as GrokApiBackend })}><option value="responses">Responses API</option><option value="chat_completions">Chat Completions</option><option value="messages">Messages API</option></select></Label><Label className="grok-form-row"><span>{t("上下文窗口")}</span><Input inputMode="numeric" value={selected.contextWindowText} onChange={(event) => update({ contextWindowText: event.currentTarget.value })} /></Label><Label className="grok-form-row"><span>API Key</span><Input disabled={selected.removeApiKey} type="password" value={selected.apiKeyUpdate} placeholder={selected.apiKeyConfigured ? t("留空保持当前 Key") : t("输入 API Key")} onChange={(event) => update({ apiKeyUpdate: event.currentTarget.value })} /></Label>{selected.apiKeyConfigured ? <label className="inline-check"><input checked={selected.removeApiKey} type="checkbox" onChange={(event) => update({ removeApiKey: event.currentTarget.checked, apiKeyUpdate: event.currentTarget.checked ? "" : selected.apiKeyUpdate })} /><span>{t("移除当前 API Key")}</span></label> : null}<Button variant="ghost" onClick={remove}><Trash2 className="h-4 w-4" />{t("删除模型")}</Button></div> : <div className="empty">{t("选择或新增一个 Grok 模型")}</div>}</section></div>
    <div className="settings-save-bar grok-save-bar"><span className={error ? "is-error" : ""}>{error || (dirty ? t("Grok 配置有未保存修改") : t("Grok 配置已保存"))}</span><div className="toolbar"><Button variant="secondary" disabled={saving || !dirty} onClick={discard}><RotateCcw className="h-4 w-4" />{t("放弃修改")}</Button><Button variant="secondary" disabled={saving} onClick={() => void refresh()}><RefreshCw className="h-4 w-4" />{t("刷新")}</Button><Button disabled={saving || !dirty || Boolean(error)} onClick={() => void save()}><Save className="h-4 w-4" />{saving ? t("保存中") : t("保存配置")}</Button></div></div>
  </div>;
}
