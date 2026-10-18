export class Money {
  static readonly ZERO = new Money(0);

  constructor(readonly cents: number) {}

  add(other: Money): Money {
    return new Money(this.cents + other.cents);
  }

  static sum(items: Money[]): Money {
    return items.reduce((total, item) => total.add(item), Money.ZERO);
  }

  static isZero(money: Money): boolean {
    return money.cents === Money.ZERO.cents;
  }
}

export class Pool {
  private static readonly pools = new Map<string, Pool>();
  private static created = 0;

  static count(): number {
    return Pool.created;
  }

  static of(name: string): Pool {
    const pool = Pool.pools.get(name) ?? new Pool();
    Pool.pools.set(name, pool);
    Pool.created += 1;
    return pool;
  }
}

export class Session {
  private instance: Session | null = null;

  current(): Session {
    this.instance ??= new Session();
    return this.instance;
  }
}

export const cache = new Money(1);

declare class Ambient {
  static instance: Ambient;
  static getInstance(): Ambient;
}

export const ambient = Ambient.getInstance();

export class Color {
  static readonly RED = new Color('red');
  static readonly BLUE = new Color('blue');

  private constructor(readonly name: string) {}

  static parse(text: string): Color {
    return text === 'red' ? Color.RED : Color.BLUE;
  }
}

export class Theme {
  static current: Theme | string = 'light';

  static active(): Theme | string {
    return Theme.current;
  }
}
