import assert from "node:assert/strict";
import test from "node:test";

import { completeContextDelete } from "./context-delete-transaction.ts";

test("删除持久化失败时不更新本地状态也不发送 live tombstone", async () => {
  const events: string[] = [];

  const completed = await completeContextDelete({
    deleteAndPersist: async () => {
      events.push("delete", "save-failed");
      return null;
    },
    updateLocal: () => events.push("local"),
    syncLive: async () => {
      events.push("live-tombstone");
    },
  });

  assert.equal(completed, false);
  assert.deepEqual(events, ["delete", "save-failed"]);
});

test("删除成功时先持久化，再更新本地状态并同步 live tombstone", async () => {
  const events: string[] = [];
  const savedSettings = { relayContextConfigContents: "" };

  const completed = await completeContextDelete({
    deleteAndPersist: async () => {
      events.push("delete", "save");
      return savedSettings;
    },
    updateLocal: (settings) => {
      assert.equal(settings, savedSettings);
      events.push("local");
    },
    syncLive: async (settings) => {
      assert.equal(settings, savedSettings);
      events.push("live-tombstone");
    },
  });

  assert.equal(completed, true);
  assert.deepEqual(events, ["delete", "save", "local", "live-tombstone"]);
});
