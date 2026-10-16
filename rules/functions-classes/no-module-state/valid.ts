import { Logger } from './logger';

const ROUTES = new Map<string, string>([['orders', '/orders']]);
const STATUSES = new Set(['open', 'paid']);
const DEFAULTS = { retries: 3 };
const logger = new Logger('orders');
const client = createClient();

ROUTES.set('refunds', '/refunds');

declare let ambient: number;
declare function createClient(): { set(key: string, value: string): void };

export class Orders {
  private readonly cache = new Map<string, number>();

  route(name: string): string | undefined {
    let found = ROUTES.get(name);
    found ??= STATUSES.has(name) ? name : undefined;
    this.cache.set(name, DEFAULTS.retries + ambient);
    client.set(name, 'seen');
    logger.log(name);
    return found;
  }
}
