declare function Inject(token: string): ParameterDecorator;
declare function Column(): PropertyDecorator;
declare function Get(path: string): MethodDecorator;

export class OrderService {
  constructor(@Inject('db') private readonly db: string) {}

  // How many orders this instance has seen.
  private count = 0; // expect: class-member-order

  total(): string {
    return `${this.db}:${this.count}:${this.label}`;
  }

  /** The service's label. */
  readonly label = 'orders';
}

export class Prices {
  format(price: number): string;
  format(price: string): string;
  @Get('format')
  format(price: number | string): string {
    return `${price} ${this.currency} ${Prices.DEFAULT}`;
  }

  constructor(@Inject('currency') private readonly currency: string) {} // expect: class-member-order

  static readonly DEFAULT = 'USD';
}

export class Registry {
  static register(name: string): void {
    Registry.names.push(name);
  }

  static names: string[] = []; // expect: class-member-order

  static {
    Registry.register('default');
  }

  [key: string]: unknown;
  declare readonly kind: string;
}

export class Vault {
  [Symbol.iterator](): Iterator<string> {
    return [this.secret][Symbol.iterator]();
  }

  private readonly secret = 'secret'; // expect: class-member-order
}

export class OrderView {
  @Get('id')
  id(): string {
    return this.name;
  }

  @Column()
  name = ''; // expect: class-member-order
}

export class Ids {
  *ids(): Generator<string> {
    yield this.prefix
  }

  prefix = 'id' // expect: class-member-order
}

export class Point { move(): number { return this.x; } x = 0; } // expect: class-member-order

export class Counter {
  increment(): number {
    return ++this.value;
  }

  accessor value = 0; // expect: class-member-order
}

export class Legacy {
  /* eslint-disable no-console */
  run(): void {
    console.log(this.name);
  }
  /* eslint-enable no-console */

  name = 'legacy'; // expect: class-member-order
}

export class Report {
  // #region output
  render(): string {
    return this.title;
  }
  // #endregion

  title = 'report'; // expect: class-member-order
}

export class Archive {
  // biome-ignore-start lint/suspicious/noConsole: legacy output
  render(): string {
    return this.label;
  }
  // biome-ignore-end lint/suspicious/noConsole: legacy output

  label = 'archive'; // expect: class-member-order
}

export class Draft { // eslint-disable-next-line class-methods-use-this
  render(): string {
    return this.body;
  }

  body = 'draft'; // expect: class-member-order
}

export class Tagged {
  read(): string {
    return this.tag;
  }

  @Column() // stored as text
  tag = ''; // expect: class-member-order
}
