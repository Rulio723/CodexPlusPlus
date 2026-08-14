import assert from "node:assert";
import { describe, it } from "node:test";
import { officialAccountsAfterProviderSwitch } from "./official-accounts.ts";

describe("official accounts after provider switch", () => {
  it("clears the current label and makes every account switchable", () => {
    const result = officialAccountsAfterProviderSwitch({
      status: "ok",
      message: "loaded",
      currentAccountLabel: "alice@example.com",
      accounts: [
        { id: "alice", active: true },
        { id: "bob", active: false },
      ],
    });

    assert.equal(result?.currentAccountLabel, null);
    assert.deepEqual(result?.accounts.map((account) => account.active), [false, false]);
    assert.equal(result?.status, "ok");
    assert.equal(result?.message, "loaded");
  });

  it("preserves an unloaded inventory", () => {
    assert.equal(officialAccountsAfterProviderSwitch(null), null);
  });
});
