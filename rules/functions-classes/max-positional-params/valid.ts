declare function Injectable(): ClassDecorator;
declare function Inject(token: string): ParameterDecorator;
declare function Param(name: string): ParameterDecorator;
declare function Query(name: string): ParameterDecorator;
declare function legacy(first: string, second: string, third: string): void;

export interface Store {
  save(key: string, value: string, ttlMs: number): void;
}

export type Compare = (left: number, right: number, scale: number) => number;

export function transfer(
  from: string,
  to: string,
  { amount, note = '' }: { amount: number; note?: string },
): string {
  return `${from}->${to}:${amount}${note}`;
}

export function weighted(prices: number[]): number {
  return prices.reduce((sum, price, index) => sum + price * index, 0);
}

export const moves = {
  move(from: string, to: string, amount: number): string {
    return `${from}->${to}:${amount}`;
  },
};

export function bound(this: { base: number }, value: number, factor: number): number {
  return this.base + value * factor;
}

@Injectable()
export class Orders {
  constructor(
    private readonly store: Store,
    @Inject('clock') private readonly now: () => number,
    @Inject('prefix') prefix: string,
  ) {
    this.store.save(prefix, String(this.now()), 0);
  }

  find(@Param('id') id: string, @Query('page') page: string, @Query('size') size: string): string {
    legacy(id, page, size);
    return id;
  }
}

export class Money {
  constructor(
    readonly amount: number,
    readonly currency: string,
    readonly scale: number,
  ) {}
}

export class MemoryStore implements Store {
  save(key: string, value: string, ttlMs: number): void {
    this.log(`${key}=${value} for ${ttlMs}`);
  }

  private log(line: string): void {
    void line;
  }
}

export abstract class Job {
  abstract run(first: string, second: string, third: string): void;
}

export class ReportJob extends Job {
  override run(first: string, second: string, third: string): void {
    void [first, second, third];
  }
}

export const compare: Compare = (left, right, scale) => (left - right) * scale;

export type Handler = (request: string, response: string, next: () => void) => void;

export function audit(prefix: string): Handler {
  return (request, response, next) => {
    void [prefix, request, response];
    next();
  };
}

export const trace =
  (prefix: string): Handler =>
  (request, response, next) => {
    void [prefix, request, response];
    next();
  };

export const relay = ((request: string, response: string, next: () => void) => {
  void [request, response];
  next();
}) satisfies Handler;

export class FieldStore implements Store {
  save = (key: string, value: string, ttlMs: number): void => {
    void [key, value, ttlMs];
  };
}
