declare function Inject(token: string): ParameterDecorator;
declare function Column(): PropertyDecorator;
declare function Get(path: string): MethodDecorator;

export class OrderService {
  // How many orders this instance has seen.
  private count = 0;

  /** The service's label. */
  readonly label = 'orders';

  constructor(@Inject('db') private readonly db: string) {}

  total(): string {
    return `${this.db}:${this.count}:${this.label}`;
  }
}

export class Prices {
  static readonly DEFAULT = 'USD';

  constructor(@Inject('currency') private readonly currency: string) {}

  format(price: number): string;
  format(price: string): string;
  @Get('format')
  format(price: number | string): string {
    return `${price} ${this.currency} ${Prices.DEFAULT}`;
  }
}

export class Registry {
  static names: string[] = [];

  static {
    Registry.register('default');
  }

  [key: string]: unknown;
  declare readonly kind: string;

  static register(name: string): void {
    Registry.names.push(name);
  }
}

export class Vault {
  private readonly secret = 'secret';

  [Symbol.iterator](): Iterator<string> {
    return [this.secret][Symbol.iterator]();
  }
}

export class OrderView {
  @Get('id')
  id(): string {
    return this.name;
  }

  @Column()
  name = '';
}

export class Ids {
  *ids(): Generator<string> {
    yield this.prefix
  }

  prefix = 'id'
}

export class Point { move(): number { return this.x; } x = 0; }

export class Counter {
  increment(): number {
    return ++this.value;
  }

  accessor value = 0;
}

export class Legacy {
  /* eslint-disable no-console */
  run(): void {
    console.log(this.name);
  }
  /* eslint-enable no-console */

  name = 'legacy';
}
