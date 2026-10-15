import type { Stats } from 'node:fs';
import type { Request } from 'express';
import type { Order } from './order.types';

declare function load(): Promise<Order>;

export type StatsPatch = Partial<Stats>;
export type RequestView = Pick<Request, 'url' | 'method'>;
export type Settings = Partial<Record<string, number>>;
export type GlobalPatch = Partial<Express.Request>;
export type OrderKeys = Exclude<keyof Order, 'id'>;
export type Loaded = Awaited<ReturnType<typeof load>>;
export type LoadArgs = Parameters<typeof load>;
export type OrderWithTotal = Order & { total: number };
export type Merged = Order & Request;
export type Mapped<T> = { [K in keyof T]: Exclude<K, 'id'> };

export interface Line extends Order {
  sku: string;
}

export function merge<T>(base: T, patch: Partial<T>): T {
  return { ...base, ...patch };
}
