type Row = { price: number; qty: number };

function logged(target: object, key: string): void {
  console.log(`decorated ${key}`);
}

export class Orders {
  @logged
  list(): Row[] {
    return [];
  }
}

export function total(rows: Row[]): number {
  let sum = 0;
  for (const row of rows) {
    sum += row.price * row.qty;
  }
  return sum;
}

export function report(rows: Row[]): string {
  return `total ${total(rows)}`;
}

export function fib(n: number): number {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
}
