export type AdministratorCapabilityState = "elevated" | "notElevated";

export type AdministratorLaunchStatusSnapshot = {
  started_at_ms: number;
  administrator_mode: {
    requested: boolean;
    state: string;
  };
};

export function administratorCapabilityState(elevated: boolean): AdministratorCapabilityState {
  return elevated ? "elevated" : "notElevated";
}

export function administratorLaunchStatusSettled(
  previousStartedAtMs: number,
  expectedAdministratorMode: boolean,
  latest: AdministratorLaunchStatusSnapshot | null,
): boolean {
  if (!latest) return false;

  const administratorMode = latest.administrator_mode;
  if (latest.started_at_ms <= previousStartedAtMs) {
    return false;
  }

  if (!expectedAdministratorMode) {
    return !administratorMode.requested && administratorMode.state === "off";
  }

  return administratorMode.requested
    && (administratorMode.state === "active" || administratorMode.state === "failed");
}
