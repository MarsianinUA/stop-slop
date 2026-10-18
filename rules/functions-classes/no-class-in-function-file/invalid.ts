// @path src/orders/price.util.ts
export class PriceFormatter { // expect: no-class-in-function-file
  format(price: number): string {
    return price.toFixed(2);
  }
}

class RoundingError extends Error {} // expect: no-class-in-function-file

export const Rounding = class { // expect: no-class-in-function-file
  round(price: number): number {
    if (Number.isNaN(price)) {
      throw new RoundingError('NaN');
    }
    return Math.round(price);
  }
};

export function formatPrice(price: number): string {
  return new PriceFormatter().format(price);
}
