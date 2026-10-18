export function kind(code: number): string {
  switch (code) {
    case 1: {
      const name = 'one';
      return name;
    }
    case 2: {
      return 'two';
    }
    default:
      return 'other';
  }
}

export function find(rows: number[][]): number {
  let found = -1;
  outer: {
    for (const row of rows) {
      if (row.length === 0) {
        break outer;
      }
      found = row[0];
    }
  }
  try {
    found += 1;
  } catch {
    found = 0;
  } finally {
    found += 0;
  }
  return found;
}

export const config = { retries: 3 };
export const empty = (): void => {};

export class Store {
  static count = 0;
  static {
    Store.count = 1;
  }
}
