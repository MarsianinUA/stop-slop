// @path src/orders/order.response.ts
export interface OrderResponse {
  id: string;
  note?: string; // expect: no-optional-in-emitted
  shipping: {
    trackingCode?: string; // expect: no-optional-in-emitted
  };
}

export type OrderSummary = {
  total?: number; // expect: no-optional-in-emitted
};

export class OrderLine {
  discount?: number; // expect: no-optional-in-emitted
}

interface Draft {
  hint?: string;
}

export function toResponse(draft: Draft, options?: { verbose?: boolean }): OrderResponse {
  return { id: draft.hint ?? String(options?.verbose), shipping: {} };
}
