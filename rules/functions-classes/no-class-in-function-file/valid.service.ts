// @path src/orders/price.service.ts
export class PriceService {
  format(price: number): string {
    return price.toFixed(2);
  }
}
