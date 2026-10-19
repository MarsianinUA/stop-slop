interface Order {
  id: string;
  total: number;
}

interface Tally {
  status: string;
  count: number;
}

declare const orders: Order[];
declare const pages: Order[][];

export function perPass(): number[] {
  const sizes: number[] = [];
  for (const order of orders) {
    let row: { [key: string]: number } = {};
    row = { ...row, [order.id]: order.total };
    sizes.push(Object.keys(row).length);
  }
  return sizes;
}

export function pushed(): Order[] {
  const all: Order[] = [];
  for (const page of pages) all.push(...page);
  return all;
}

export function fixedKeys(): Tally {
  let tally: Tally = { status: 'new', count: 0 };
  for (const order of orders) tally = { ...tally, count: tally.count + order.total };
  return tally;
}

export function replaced(): Order[] {
  let last: Order[] = [];
  for (const page of pages) last = [...page, ...orders];
  return last;
}

export function once(extra: Order): Order[] {
  let all = [...orders];
  all = [...all, extra];
  return all;
}

export function text(): string {
  let line = '';
  for (const order of orders) line = line.concat(order.id);
  return line;
}

export function later(): void {
  let all: Order[] = [];
  for (const page of pages) {
    setTimeout(() => {
      all = [...all, ...page];
    });
  }
}

export function rebound(): void {
  for (let order of orders) {
    order = { ...order, [order.id]: order.total };
  }
}
