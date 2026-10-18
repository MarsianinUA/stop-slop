// @path src/orders.test.ts
interface Order { readonly id: string }

const source = { id: 'first' };
const widened: unknown = source;
const parsed = widened as Order;

export { parsed };
