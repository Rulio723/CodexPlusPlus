export type ContextDeleteTransaction<T> = {
  deleteAndPersist: () => Promise<T | null>;
  updateLocal: (settings: T) => void;
  syncLive: (settings: T) => Promise<unknown>;
};

/**
 * 只有删除结果已写入 settings 后，才更新本地状态并同步 live config。
 * 这样保存失败时，删除 tombstone 不会被提前发送到 live config。
 */
export async function completeContextDelete<T>({
  deleteAndPersist,
  updateLocal,
  syncLive,
}: ContextDeleteTransaction<T>): Promise<boolean> {
  const persisted = await deleteAndPersist();
  if (!persisted) return false;

  updateLocal(persisted);
  await syncLive(persisted);
  return true;
}
