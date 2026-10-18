// @path src/orders/price.util.ts
export class PriceFormat {}

export function formatPrice(price: number): string {
  return price.toFixed(2);
}
