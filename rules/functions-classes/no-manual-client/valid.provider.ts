// @path src/orders/order-client.provider.ts
import { HttpClient } from './clients';

export class OrderClientFactory {
  create(url: string): HttpClient {
    return new HttpClient(url);
  }
}
