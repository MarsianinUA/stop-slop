// @path src/orders/order.service.ts
import { createClient, HttpClient, Pool } from './clients';

declare function Injectable(): ClassDecorator;

@Injectable()
export class OrderService {
  private readonly http = new HttpClient('https://orders.example'); // expect: no-manual-client
  private readonly cache = createClient({ url: 'redis://cache' }); // expect: no-manual-client

  async count(): Promise<number> {
    const pool = new Pool({ max: 2 }); // expect: no-manual-client
    return pool.size + this.http.timeoutMs + this.cache.size;
  }
}

export class OrderCache {
  readonly pool = new Pool({ max: 1 }); // expect: no-manual-client
}
