// @path src/orders/price.ts
export function formatPrice(price: number): string {
  return price.toFixed(2);
}

export function makeFormatter(): { format(price: number): string } {
  class Formatter {
    format(price: number): string {
      return formatPrice(price);
    }
  }
  return new Formatter();
}

declare class Legacy {}
export const legacy: Legacy | null = null;
