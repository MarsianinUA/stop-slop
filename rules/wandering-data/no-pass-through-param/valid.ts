import { saveOrder } from './orders.repository';

interface Order {
  id: string;
  total: number;
  children: Order[];
}

interface Store {
  keep(order: Order): void;
}

declare function Body(): ParameterDecorator;
declare function emit(order: Order): void;

async function persist(order: Order): Promise<void> {
  await saveOrder(order);
}

function describe(order: Order): string {
  return `${order.id}:${order.total}`;
}

export function label(order: Order, prefix: string): string {
  const text = describe(order);
  return `${prefix}${text}`;
}

export function publish(order: Order): void {
  emit(order);
  void persist(order);
}

export function remember(order: Order, key: string, index: Map<string, Order>): void {
  index.set(key, order);
  void persist(order);
}

export async function store(order: Order): Promise<void> {
  await persist(order);
}

function mark(order: Order, seen: Set<string>): void {
  seen.add(order.id);
}

export function visitAll(orders: Order[], seen: Set<string>): void {
  for (const order of orders) {
    mark(order, seen);
    visitAll(order.children, seen);
  }
}

export function storeEach(orders: Order[]): void {
  orders.forEach((order) => {
    const started = Date.now();
    void persist(order);
    void started;
  });
}

export async function loadAll(ids: string[], signal: AbortSignal): Promise<string[]> {
  const results = await Promise.all(ids.map((id) => fetchOne(id, signal)));
  return results;
}

async function fetchOne(id: string, signal: AbortSignal): Promise<string> {
  await fetchText(id, signal);
  return id;
}

async function fetchText(id: string, signal: AbortSignal): Promise<string> {
  const response = await fetch(`/orders/${id}`, { signal });
  return response.text();
}

export class Orders implements Store {
  constructor(private readonly store: Store) {}

  keep(order: Order): void {
    const started = Date.now();
    this.save(order);
    void started;
  }

  private save(order: Order): void {
    this.store.keep(order);
  }
}

export class OrdersController {
  constructor(private readonly orders: Orders) {}

  create(@Body() order: Order): void {
    const started = Date.now();
    this.keep(order);
    void started;
  }

  private keep(order: Order): void {
    this.orders.keep(order);
  }
}

export class Draft {
  constructor(private readonly order: Order) {
    void persist(order);
  }

  id(): string {
    return this.order.id;
  }
}

interface Queue {
  start(): void;
}

declare function Injectable(): ClassDecorator;

@Injectable()
export class Jobs {
  constructor(queue: Queue) {
    const started = Date.now();
    this.listen(queue);
    void started;
  }

  private listen(queue: Queue): void {
    this.attach(queue);
  }

  private attach(queue: Queue): void {
    queue.start();
  }
}

interface Executor {
  run(sql: string): void;
}

interface Logger {
  info(line: string): void;
}

function insert(tx: Executor, sql: string): void {
  tx.run(sql);
}

function write(tx: Executor, sql: string): void {
  insert(tx, sql);
}

export function saveAll(tx: Executor, sql: string): string {
  const started = `${sql}:${Date.now()}`;
  write(tx, sql);
  return started;
}

function report(log: Logger, line: string): void {
  log.info(line);
}

function relay(log: Logger, line: string): void {
  report(log, line);
}

export function announce(log: Logger, line: string): string {
  const text = line.trim();
  relay(log, text);
  return text;
}
