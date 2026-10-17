const BASE_CODES = { Unknown: 10800 } as const;

export enum OrderErrorCode {
  NotFound = 10701,
  Conflict = 0x29ee,
  Retired = -1,
}

export enum OrderStatus {
  Open,
  Closed,
}

export enum ErrorCodeKind {
  Domain = 'domain',
}

declare enum VendorErrorCode {
  Timeout,
}

export const PRICE_ERROR_CODES = {
  ...BASE_CODES,
  Stale: 10_801,
} as const;

export const errorCodes = ['E1', 'E2'];

export type HttpErrorCode = 'bad-request' | 'not-found';

export { VendorErrorCode };
