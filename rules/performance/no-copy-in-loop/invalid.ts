interface Order {
  id: string;
}

declare const orders: Order[];
declare const pages: Order[][];
declare const stream: { on(event: 'data', listener: (chunk: Buffer) => void): void };
declare const chunks: AsyncIterable<Buffer>;

export function spread(): Order[] {
  let all: Order[] = [];
  for (const page of pages) {
    all = [...all, ...page]; // expect: no-copy-in-loop
  }
  return all;
}

export function concatenated(): Order[] {
  let all = [];
  for (let i = 0; i < pages.length; i++) all = all.concat(pages[i]); // expect: no-copy-in-loop
  return all;
}

export function indexed(): { [id: string]: Order } {
  let byId: { [id: string]: Order } = {};
  orders.forEach((order) => {
    byId = { ...byId, [order.id]: order }; // expect: no-copy-in-loop
  });
  return byId;
}

export function listened(): void {
  let buffer = Buffer.alloc(0);
  stream.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]); // expect: no-copy-in-loop
  });
}

export async function streamed(): Promise<Buffer> {
  let buffer = Buffer.alloc(0);
  for await (const chunk of chunks) {
    buffer = Buffer.concat([buffer, chunk]); // expect: no-copy-in-loop
  }
  return buffer;
}

export function prepended(limit: number): number[] {
  let seen: number[] = [];
  let i = 0;
  while (i < limit) {
    seen = [i, ...seen]; // expect: no-copy-in-loop
    i += 1;
  }
  return seen;
}

export function perPage(): number[] {
  const sizes: number[] = [];
  for (const page of pages) {
    let byId: { [id: string]: Order } = {};
    for (const order of page) byId = { ...byId, [order.id]: order }; // expect: no-copy-in-loop
    sizes.push(Object.keys(byId).length);
  }
  return sizes;
}

export function headed(): string[] {
  const out: string[] = [];
  for (let ids: string[] = [], i = 0; i < orders.length; i++) {
    ids = [...ids, orders[i].id]; // expect: no-copy-in-loop
    out.push(ids.join());
  }
  return out;
}
