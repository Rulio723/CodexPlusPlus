import { CircleArrowUp, Download, Github, Plus, RefreshCw, Search, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { t, tf } from "@/i18n";

type Status = "ok" | "failed" | "not_implemented" | "not_checked" | string;
type CommandResult<T> = T & { status: Status; message: string };
export type SkillRepo = { owner: string; name: string; branch: string; subdir: string; enabled: boolean };
type SkillEntry = { id: string; name: string; description: string; repoKey: string; installed: boolean; enabled: boolean; bundled: boolean; updateAvailable: boolean };
type SkillBackup = { id: string; skillId: string; name: string; backedUpAt: string };
export type SkillsResult = CommandResult<{ skills: SkillEntry[]; repos: SkillRepo[]; backups: SkillBackup[]; repoErrors: string[]; skillsDir: string; codexSkillsDir: string }>;
export type RemoteSkillsActions = {
  refreshSkillCatalog: (silent?: boolean) => Promise<void>;
  installSkill: (repoKey: string, id: string) => Promise<void>;
  updateSkill: (repoKey: string, id: string) => Promise<void>;
  setSkillEnabled: (id: string, enabled: boolean) => Promise<void>;
  uninstallSkill: (id: string) => Promise<void>;
  restoreSkillBackup: (backupId: string) => Promise<void>;
  deleteSkillBackup: (backupId: string) => Promise<void>;
  upsertSkillRepo: (repo: SkillRepo) => Promise<SkillsResult | null>;
  deleteSkillRepo: (key: string) => Promise<void>;
  skillBusyId: string | null;
};
const repoKey = (repo: SkillRepo) => `${repo.owner}/${repo.name}@${repo.branch}${repo.subdir ? `:${repo.subdir}` : ""}`;

export function RemoteSkillsScreen({ skills, actions }: { skills: SkillsResult | null; actions: RemoteSkillsActions }) {
  const [query, setQuery] = useState("");
  const [reposOpen, setReposOpen] = useState(false);
  const [backupsOpen, setBackupsOpen] = useState(false);
  const entries = skills?.skills ?? [];
  const repoErrors = skills?.repoErrors ?? [];
  const visible = useMemo(() => entries.filter((entry) => [entry.id, entry.name, entry.description, entry.repoKey].join(" ").toLowerCase().includes(query.trim().toLowerCase())), [entries, query]);
  const updates = entries.filter((entry) => entry.updateAvailable);
  return <>
    <section className="panel"><div className="panel-head"><h2>{t("Skills 技能")}</h2><p>{t("从远程仓库安装 Skill，启用后链接到 Codex Skills 目录。")}</p></div><div className="panel-content"><div className="metric-list"><span>{tf("可安装 {0}", [entries.length])}</span><span>{tf("已安装 {0}", [entries.filter((entry) => entry.installed).length])}</span><span>{tf("可更新 {0}", [updates.length])}</span></div><div className="toolbar"><Button onClick={() => void actions.refreshSkillCatalog()}><RefreshCw className="h-4 w-4" />{t("刷新列表")}</Button>{updates.length ? <Button variant="secondary" onClick={() => void updates.reduce(async (previous, entry) => { await previous; await actions.updateSkill(entry.repoKey, entry.id); }, Promise.resolve())}><CircleArrowUp className="h-4 w-4" />{t("全部更新")}</Button> : null}<Button variant="secondary" onClick={() => setReposOpen(!reposOpen)}><Github className="h-4 w-4" />{t("仓库管理")}</Button><Button variant="secondary" onClick={() => setBackupsOpen(!backupsOpen)}><Download className="h-4 w-4" />{tf("备份（{0}）", [skills?.backups.length ?? 0])}</Button></div>{skills ? <p className="relay-context-summary">{tf("源目录 {0}；启用后链接到 {1}", [skills.skillsDir, skills.codexSkillsDir])}</p> : null}{repoErrors.length ? <div className="relay-context-summary" role="alert"><strong>{t("以下仓库拉取失败，当前显示上次成功的目录结果：")}</strong><ul>{repoErrors.map((error, index) => <li key={`${index}-${error}`}>{error}</li>)}</ul></div> : null}</div></section>
    {reposOpen ? <RepoManager repos={skills?.repos ?? []} actions={actions} /> : null}{backupsOpen ? <BackupManager backups={skills?.backups ?? []} actions={actions} /> : null}
    <section className="panel"><div className="panel-head"><h2>{t("技能列表")}</h2></div><div className="panel-content"><div className="script-market-search"><Search className="h-4 w-4" /><Input value={query} placeholder={t("搜索名称、描述或仓库")} onChange={(event) => setQuery(event.currentTarget.value)} /></div><div className="script-market-grid">{visible.map((entry) => <SkillCard key={entry.id} entry={entry} actions={actions} />)}</div>{!visible.length ? <div className="empty">{t("还没有可显示的 Skill。")}</div> : null}</div></section>
  </>;
}

function SkillCard({ entry, actions }: { entry: SkillEntry; actions: RemoteSkillsActions }) { const busy = actions.skillBusyId === entry.id; return <div className="skill-card"><div className="skill-card-title"><strong>{entry.name || entry.id}</strong><span className="skill-card-source">{entry.repoKey || t("本地")}</span></div><p className="skill-card-description">{entry.description || t("暂无描述。")}</p><div className="skill-card-actions">{entry.bundled ? <span>{t("Codex 内置")}</span> : entry.installed ? <><Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.setSkillEnabled(entry.id, !entry.enabled)}>{entry.enabled ? t("停用") : t("启用")}</Button>{entry.updateAvailable ? <Button size="sm" disabled={busy} onClick={() => void actions.updateSkill(entry.repoKey, entry.id)}>{t("更新")}</Button> : null}<Button size="sm" variant="ghost" disabled={busy} onClick={() => void actions.uninstallSkill(entry.id)}><Trash2 className="h-4 w-4" />{t("卸载")}</Button></> : <Button size="sm" disabled={busy || !entry.repoKey} onClick={() => void actions.installSkill(entry.repoKey, entry.id)}><Download className="h-4 w-4" />{t("安装")}</Button>}</div></div>; }
function RepoManager({ repos, actions }: { repos: SkillRepo[]; actions: RemoteSkillsActions }) { const [draft, setDraft] = useState<SkillRepo>({ owner: "", name: "", branch: "main", subdir: "", enabled: true }); return <section className="panel"><div className="panel-head"><h2>{t("仓库源")}</h2></div><div className="panel-content"><div className="relay-context-list">{repos.map((repo) => <div className="relay-context-row" key={repoKey(repo)}><strong>{repoKey(repo)}</strong><div className="relay-context-actions"><Button size="sm" variant="secondary" onClick={() => void actions.upsertSkillRepo({ ...repo, enabled: !repo.enabled })}>{repo.enabled ? t("停用") : t("启用")}</Button><Button size="icon" variant="ghost" onClick={() => void actions.deleteSkillRepo(repoKey(repo))}><Trash2 className="h-4 w-4" /></Button></div></div>)}</div><div className="context-editor-fields">{(["owner", "name", "branch", "subdir"] as const).map((field) => <Input key={field} value={draft[field]} placeholder={field} onChange={(event) => setDraft({ ...draft, [field]: event.currentTarget.value })} />)}</div><Button disabled={!draft.owner.trim() || !draft.name.trim()} onClick={() => void actions.upsertSkillRepo(draft)}><Plus className="h-4 w-4" />{t("添加仓库源")}</Button></div></section>; }
function BackupManager({ backups, actions }: { backups: SkillBackup[]; actions: RemoteSkillsActions }) { return <section className="panel"><div className="panel-head"><h2>{t("卸载备份")}</h2></div><div className="panel-content relay-context-list">{backups.map((backup) => <div className="relay-context-row" key={backup.id}><div><strong>{backup.name || backup.skillId}</strong><small>{backup.backedUpAt}</small></div><div className="relay-context-actions"><Button size="sm" variant="secondary" disabled={actions.skillBusyId === backup.id} onClick={() => void actions.restoreSkillBackup(backup.id)}>{t("恢复")}</Button><Button size="icon" variant="ghost" disabled={actions.skillBusyId === backup.id} onClick={() => void actions.deleteSkillBackup(backup.id)}><Trash2 className="h-4 w-4" /></Button></div></div>)}{!backups.length ? <div className="empty">{t("还没有备份。")}</div> : null}</div></section>; }
