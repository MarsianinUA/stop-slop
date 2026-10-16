export class OrderRepository {}
export class CacheManagerConfig {}

export function handleOrderCreated(): void {}

export function trim(rows: string[]): string[] {
  return rows.map((item) => item.trim());
}

export function lengths(rows: string[]): number {
  let total = 0;
  for (const item of rows) {
    total += item.length;
  }
  return total;
}

export function read(data: string): string {
  const { result } = { result: data };
  return result;
}

export class Store {
  baseUrl = '';
  protected helper(): string {
    return this.baseUrl;
  }
}

export const response = { data: 1, result: 2 };
