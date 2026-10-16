import { Logger } from '@nestjs/common';

export const PAGE_SIZE = 50;
export const DEFAULT_TIMEOUT_MS = 60 * 1000;
export const ORDER_KINDS = ['retail', 'bulk'] as const;
export const OrderStatus = { Open: 'open', Closed: 'closed' } as const;
export const logger = new Logger('orders');
export const ORDERS_TOKEN = Symbol('orders');

type OrderRow = { id: string; created_at: string };
interface LineItem {
  sku: string;
}

export function Public(): MethodDecorator {
  return () => undefined;
}
export const Roles = (...roles: string[]): string[] => roles;
export const orders$ = [PAGE_SIZE, DEFAULT_TIMEOUT_MS];

export function build<Model>(Target: new () => Model): Model {
  return new Target();
}

export async function loadModules(): Promise<unknown[]> {
  const [{ CacheModule }, { MAX_ITEMS }] = await Promise.all([
    import('./cache.module'),
    import('./cache.constants'),
  ]);
  const MockLogger = build(Object);
  return [CacheModule, MAX_ITEMS, MockLogger];
}

export function totals(rows: OrderRow[], _unused: LineItem): number {
  const { created_at: createdAt } = rows[0];
  let count = 0;
  for (const row of rows) {
    count += row.id.length + createdAt.length;
  }
  return count;
}

export class Cart {
  private itemCount = 0;
  static readonly MAX_ITEMS = 10;
  protected Legacy_Total = 0;
  #lastSku = '';

  add(sku: string): number {
    this.#lastSku = sku;
    this.itemCount += Cart.MAX_ITEMS + this.Legacy_Total;
    return this.itemCount;
  }
}

declare const __DEV__: boolean;
declare global {
  interface Window { app_state: unknown }
}
export const debugEnabled = __DEV__;

export class Statuses {
  private static readonly ORDER_STATES = ['open', 'paid'] as const;

  static first(): string {
    return Statuses.ORDER_STATES[0];
  }
}

declare class Admin {}
declare class Guest {}

export function pick(isAdmin: boolean): object {
  const Model = isAdmin ? Admin : Guest;
  return new Model();
}
