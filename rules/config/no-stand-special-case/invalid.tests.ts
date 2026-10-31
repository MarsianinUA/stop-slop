// @path src/orders/orders.service.spec.ts
export function onStand(): boolean {
  return process.env.STOP_SLOP_STAND === '1'; // expect: no-stand-special-case
}
