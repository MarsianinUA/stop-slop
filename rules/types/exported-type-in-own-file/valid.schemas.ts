// @path src/orders/order.schemas.ts
declare function object<T>(shape: T): T;

export const orderSchema = object({ id: '' });

export type OrderInput = typeof orderSchema;
