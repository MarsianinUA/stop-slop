// @path src/orders/__tests__/orders.service.spec.ts
interface OrderRepository {
  find(id: string): string;
}

export const repository: Partial<OrderRepository> = {};
