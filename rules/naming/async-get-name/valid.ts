declare function query(sql: string): Promise<string[]>;
declare function Controller(path: string): ClassDecorator;
declare function Get(): MethodDecorator;
declare const redis: { get(key: string): Promise<string | null> };

export async function fetchOrders(): Promise<string[]> {
  return await query('select id from orders');
}

export async function loadCustomers(): Promise<string[]> {
  return query('select id from customers');
}

export class OrderCache {
  private readonly orders = new Map<string, string>();

  async get(id: string): Promise<string | null> {
    return await redis.get(id);
  }

  async getOrSet(id: string, load: () => Promise<string>): Promise<string> {
    const order = this.orders.get(id) ?? (await load());
    this.orders.set(id, order);
    return order;
  }

  async getCached(id: string): Promise<string | undefined> {
    return this.orders.get(id);
  }

  getSize(): number {
    return this.orders.size;
  }

  get count(): number {
    return this.orders.size;
  }
}

@Controller('orders')
export class OrdersController {
  @Get()
  async getOrders(): Promise<string[]> {
    return await query('select id from orders');
  }
}
