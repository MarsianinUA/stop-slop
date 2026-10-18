// @path src/orders/price.util.ts
declare class Decimal {
  toFixed(digits: number): string;
}

export function formatPrice(price: Decimal): string {
  return price.toFixed(2);
}

export function makeFormatter(): { format(price: number): string } {
  return new (class {
    format(price: number): string {
      return price.toFixed(2);
    }
  })();
}
