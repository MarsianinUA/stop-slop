interface Customer {
  name: string;
  email: string;
}

interface Order {
  id: string;
  total: number;
  customer: Customer;
  paidAt: Date | null;
}

interface PaidOrder extends Order {
  paidAt: Date;
}

interface Labeler {
  label(order: Order): string;
}

interface Logger {
  info(line: string): void;
}

interface ConnectOptions {
  timeoutMs?: number;
}

declare function Body(): ParameterDecorator;
declare function Injectable(): ClassDecorator;
declare function save(order: Order): void;

export function summary(order: Order): string {
  return `${order.id}:${order.total}`;
}

export function close(order: Order): void {
  order.total = 0;
}

export function archive(order: Order): string {
  save(order);
  return order.id;
}

export function log(logger: Logger): void {
  logger.info('done');
}

export function isPaid(order: Order): order is PaidOrder {
  return order.paidAt !== null;
}

export function size(text: string, items: Order[]): number {
  return text.length + items.length;
}

export function host(request: Request): string {
  return request.url;
}

export function connect(url: string, options: ConnectOptions): string {
  return `${url}?t=${options.timeoutMs}`;
}

export const ids = (orders: Order[]): string[] => orders.map((order: Order) => order.id);

export function byKey(order: Order, key: 'id' | 'total'): string {
  return String(order[key]);
}

export function owner({ customer }: Order): string {
  return customer.name;
}

export class OrderLabeler implements Labeler {
  label(order: Order): string {
    return order.id;
  }
}

export class OrdersController {
  constructor(private readonly order: Order) {}

  create(@Body() order: Order): string {
    return order.id + this.order.id;
  }
}

@Injectable()
export class Mailer {
  private readonly sender: string;

  constructor(customer: Customer) {
    this.sender = customer.email;
  }

  from(): string {
    return this.sender;
  }
}
