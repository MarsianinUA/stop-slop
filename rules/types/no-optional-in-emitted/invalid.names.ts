// @path src/accounts/account.mapper.ts
export class AccountSnapshotResponseDto {
  balance?: string; // expect: no-optional-in-emitted
}

export interface AccountRow {
  closedAt?: Date; // expect: no-optional-in-emitted
}

interface Settings {
  closedAt?: Date;
}

export default class AccountView {
  label?: string; // expect: no-optional-in-emitted
}

export function settingsOf(row: AccountRow): Settings {
  return { closedAt: row.closedAt };
}
