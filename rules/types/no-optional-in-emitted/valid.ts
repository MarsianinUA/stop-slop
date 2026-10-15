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

export function build(draft: Draft, options?: { verbose?: boolean }): OrderResponse {
  return { id: String(options?.verbose), note: draft.note ?? null };
}
