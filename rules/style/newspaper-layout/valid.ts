export function receipt(prices: number[]): string {
  return format(sum(prices.map(toCents)));
}

function sum(values: number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

function toCents(price: number): number {
  return Math.round(price * 100);
}

function format(cents: number): string {
  return `${cents / 100} EUR`;
}

export function head(names: string[]): string {
  return quote(names[0] ?? '');
}

function quote(name: string): string {
  return `"${name}"`;
}

export function tail(names: string[]): string {
  return quote(names[names.length - 1] ?? '');
}

function isEven(n: number): boolean {
  return n === 0 || isOdd(n - 1);
}

function isOdd(n: number): boolean {
  return n !== 0 && isEven(n - 1);
}

const parse = (value: string): number => Number(value);

export const DEFAULT_LIMIT = parse('10');

export function limit(value: string): number {
  return parse(value);
}

const currency = (): string => 'EUR';

export class Money {
  static readonly CURRENCY = currency();

  label(): string {
    return currency();
  }
}

function depth(tree: { children: { children: never[] }[] }): number {
  return 1 + Math.max(0, ...tree.children.map((child) => depth(child)));
}

export { depth, isEven };

export class Cart {
  private readonly prices: number[] = [];

  constructor() {
    this.reset();
  }

  add(price: number): void {
    this.prices.push(this.round(price));
  }

  private reset(): void {
    this.prices.length = 0;
  }

  private round(price: number): number {
    return Math.round(price);
  }

  private walk(node: { next?: object }): number {
    return node.next ? this.visit(node.next) : 0;
  }

  private visit(node: object): number {
    return 1 + this.walk(node);
  }

  static create(): Cart {
    return new Cart();
  }

  total(): number {
    return this.prices.reduce((all, price) => all + price, 0) + this.walk({});
  }
}

declare function Column(options: object): PropertyDecorator;
declare function Inject(token: string): ParameterDecorator;

const money = (): object => ({ scale: 2 });
const token = (name: string): string => `${name}-token`;
const slot = (): string => 'slot';

export class Order {
  @Column(money())
  amount = 0;

  [slot()] = 1;

  constructor(@Inject(token('repo')) readonly repo: object) {}
}
