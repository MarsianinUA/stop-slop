// @path src/config/orders.config.ts
export abstract class OrdersEnv { // expect: no-class-in-function-file
  abstract readonly url: string;
}

export function ordersConfig(): { url: string } {
  return { url: 'http://localhost' };
}
