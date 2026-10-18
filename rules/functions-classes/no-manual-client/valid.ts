// @path src/orders/order.service.ts
import { HttpClient } from './clients';

declare function Injectable(): ClassDecorator;
declare function Inject(token: string): ParameterDecorator;
declare function UseInterceptors(...interceptors: object[]): MethodDecorator;
declare class CacheClient {}
declare class Logger {
  constructor(name: string);
  log(line: string): void;
}

export function ordersHttpProvider(url: string): HttpClient {
  return new HttpClient(url);
}

@Injectable()
export class OrderService {
  private readonly logger = new Logger('orders');
  private readonly seen = new Map<string, number>();

  constructor(@Inject('HTTP') private readonly http: HttpClient) {}

  static create(url: string): OrderService {
    return new OrderService(new HttpClient(url));
  }

  createClient(name: string): string {
    return `${name}:${this.http.timeoutMs}:${this.seen.size}`;
  }

  // The decorator runs once, when the class is defined.
  @UseInterceptors(new CacheClient())
  list(): string[] {
    return [];
  }

  register(name: string): string {
    this.logger.log(name);
    return this.createClient(name);
  }
}
