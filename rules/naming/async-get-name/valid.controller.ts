// @path src/orders/orders.controller.ts
declare function query(sql: string): Promise<string[]>;

export class OrdersController {
  async getOrders(): Promise<string[]> {
    return await query('select id from orders');
  }
}
