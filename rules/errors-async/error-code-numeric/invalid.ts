const BASE = 10700;

export enum OrderErrorCode {
  NotFound, // expect: error-code-numeric
  Conflict = 'conflict', // expect: error-code-numeric
  Expired = BASE + 2, // expect: error-code-numeric
  Unknown = 10799,
}

export const enum PaymentErrorCode {
  Declined = 'PAYMENT_DECLINED', // expect: error-code-numeric
}

export const SHIPPING_ERROR_CODES = {
  lost: 'SHIPPING_LOST', // expect: error-code-numeric
  late: 10901,
} as const;

export const RefundErrorCodes = {
  Rejected: BASE, // expect: error-code-numeric
} satisfies Record<string, number>;

export const FROZEN_ERROR_CODES = Object.freeze({
  Lost: 'STOCK_LOST', // expect: error-code-numeric
  Late: 10903,
});

export const GROUPED_ERROR_CODES = {
  order: {
    Missing: 'ORDER_MISSING', // expect: error-code-numeric
    Gone: 10904,
  },
} as const;
