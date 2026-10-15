// @path src/orders/list-orders.request.dto.ts
export class ListOrdersQueryDto {
  cursor?: string;
  limit?: number;
}

export class CreateOrderDto {
  note?: string;
}
