// @path src/orders/orders.ts
import * as clients from './clients';

export class OrdersController {
  list(): number {
    return new clients.HttpClient('https://orders.example').timeoutMs; // expect: no-manual-client
  }
}

export class SyncJob {
  run(): number {
    return new clients.Pool({ max: 1 }).size; // expect: no-manual-client
  }
}

export class ReportJob {
  run(): number {
    return clients.createClient({ url: 'redis://reports' }).size; // expect: no-manual-client
  }
}
