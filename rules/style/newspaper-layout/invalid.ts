function toCents(price: number): number { // expect: newspaper-layout
  return Math.round(price * 100);
}

const format = (cents: number): string => `${cents / 100} EUR`; // expect: newspaper-layout

export function receipt(prices: number[]): string {
  return format(prices.map(toCents).reduce((sum, cents) => sum + cents, 0));
}

function isOpen(status: string): boolean { // expect: newspaper-layout
  return status === 'open';
}

export class OrderView {
  constructor(private readonly status: string) {}

  private normalize(value: string): string { // expect: newspaper-layout
    return value.trim();
  }

  describe(): string {
    return isOpen(this.normalize(this.status)) ? 'open' : 'closed';
  }

  #parse(value: string): number { // expect: newspaper-layout
    return Number(value);
  }

  get count(): number {
    return this.#parse(this.status);
  }
}

function validate(input: string): boolean { // expect: newspaper-layout
  return input !== '';
}

export const handlers = {
  create: (input: string): boolean => validate(input),
};

function label(name: string): string { // expect: newspaper-layout
  return `<${name}>`;
}

export function first(names: string[]): string {
  return names.map(label)[0] ?? '';
}

export function last(names: string[]): string {
  return label(names[names.length - 1] ?? '');
}

const shout = (text: string): string => text.toUpperCase(); // expect: newspaper-layout

export function greet(name: string): string {
  return shout(`hi ${name}`);
}

export { shout };

const shape = (): { id: number } => ({ id: 1 }); // expect: newspaper-layout

export type Shape = ReturnType<typeof shape>;

export function make(): Shape {
  return shape();
}

const tax = (amount: number): number => amount * 0.2; // expect: newspaper-layout

export class Basket {
  rate = tax(100);
}

function countdown(n: number): number { // expect: newspaper-layout
  return n <= 0 ? 0 : countdown(n - 1);
}

export function launch(): number {
  return countdown(3);
}

interface TreeNode {
  children: TreeNode[];
}

export class Tree {
  private flatten(node: TreeNode): TreeNode[] { // expect: newspaper-layout
    return [node, ...node.children.flatMap((child) => this.flatten(child))];
  }

  list(root: TreeNode): TreeNode[] {
    return this.flatten(root);
  }
}
