// @path src/config/orders.config.ts
const DEFAULT_RETRIES = 3;

export const ordersConfig = {
  retries: Number(process.env.ORDER_RETRIES ?? DEFAULT_RETRIES),
};
