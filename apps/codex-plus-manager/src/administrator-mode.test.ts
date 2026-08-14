import assert from "node:assert";
import { describe, it } from "node:test";
import {
  administratorCapabilityState,
  administratorLaunchStatusSettled,
} from "./administrator-mode.ts";

describe("administrator capability presentation", () => {
  it("distinguishes elevated and non-elevated capabilities without relying on color", () => {
    assert.equal(administratorCapabilityState(true), "elevated");
    assert.equal(administratorCapabilityState(false), "notElevated");
  });
});

describe("administrator launch status refresh", () => {
  const previousStartedAtMs = 100;

  it("waits for a newer launch instead of accepting the previous off status", () => {
    assert.equal(
      administratorLaunchStatusSettled(previousStartedAtMs, true, {
        started_at_ms: previousStartedAtMs,
        administrator_mode: { requested: true, state: "off" },
      }),
      false,
    );
  });

  it("waits for a newer launch instead of accepting the previous active status", () => {
    assert.equal(
      administratorLaunchStatusSettled(previousStartedAtMs, true, {
        started_at_ms: previousStartedAtMs,
        administrator_mode: { requested: true, state: "active" },
      }),
      false,
    );
  });

  it("waits while a new administrator launch is still starting", () => {
    assert.equal(
      administratorLaunchStatusSettled(previousStartedAtMs, true, {
        started_at_ms: previousStartedAtMs + 1,
        administrator_mode: { requested: true, state: "starting" },
      }),
      false,
    );
  });

  it("settles only after the new administrator launch is active or failed", () => {
    for (const state of ["active", "failed"]) {
      assert.equal(
        administratorLaunchStatusSettled(previousStartedAtMs, true, {
          started_at_ms: previousStartedAtMs + 1,
          administrator_mode: { requested: true, state },
        }),
        true,
      );
    }
  });

  it("settles a newer ordinary launch when administrator mode was not requested", () => {
    assert.equal(
      administratorLaunchStatusSettled(previousStartedAtMs, false, {
        started_at_ms: previousStartedAtMs + 1,
        administrator_mode: { requested: false, state: "off" },
      }),
      true,
    );
  });
});
