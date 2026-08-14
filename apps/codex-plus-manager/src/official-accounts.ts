export type OfficialAccountActivationInventory = {
  accounts: Array<{ active: boolean }>;
  currentAccountLabel?: string | null;
};

export function officialAccountsAfterProviderSwitch<
  T extends OfficialAccountActivationInventory,
>(inventory: T | null): T | null {
  if (!inventory) return null;
  return {
    ...inventory,
    currentAccountLabel: null,
    accounts: inventory.accounts.map((account) => ({ ...account, active: false })),
  } as T;
}
