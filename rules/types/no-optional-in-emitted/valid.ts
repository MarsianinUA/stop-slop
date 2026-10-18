// @path src/orders/order.response.ts
export interface OrderResponse {
  id: string;
  note: string | null;
}

export interface OrderQuery {
  cursor?: string;
}

interface Draft {
  note?: string;
}

// Only the value leaves the file; the interface of the same name stays local.
export const OrderDetails = { note: '' };
interface OrderDetails {
  note?: string;
}

// A plain constructor parameter is an input, not a field.
export class OrderPage {
  constructor(cursor?: string) {
    void cursor;
  }
}

export function build(draft: Draft, options?: { verbose?: boolean }): OrderResponse {
  return { id: String(options?.verbose), note: draft.note ?? null };
}
