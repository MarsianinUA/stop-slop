import type { Order } from './order.types';

type Partial<T> = { [K in keyof T]?: T[K] };

export type OrderPatch = Partial<Order>;
