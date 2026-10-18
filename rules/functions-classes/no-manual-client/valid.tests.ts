// @path src/orders/order.service.spec.ts
import { HttpClient } from './clients';

export class FakeOrderService {
  readonly http = new HttpClient('https://orders.test');
}
