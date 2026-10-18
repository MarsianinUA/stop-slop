export function area(w: number, h: number): number { // expect: no-single-letter-name x2
  return w * h;
}

export function total(rows: number[]): number {
  const t = rows.reduce((sum, row) => sum + row, 0); // expect: no-single-letter-name
  return t;
}

export function names(rows: { name: string }[]): string[] {
  return rows.map((r) => { // expect: no-single-letter-name
    const trimmed = r.name.trim();
    return trimmed;
  });
}

export function lengths(rows: string[]): number {
  let count = 0;
  for (const x of rows) { // expect: no-single-letter-name
    count += x.length;
  }
  return count;
}

export function pairs(map: Map<string, number>): number {
  let sum = 0;
  for (const [key, v] of map) { // expect: no-single-letter-name
    sum += key.length + v;
  }
  return sum;
}

export type K = 'key'; // expect: no-single-letter-name

export class Grid {
  private x = 0; // expect: no-single-letter-name

  move(): number {
    this.x += 1;
    return this.x;
  }
}

export const double = (x: number): number => x * 2; // expect: no-single-letter-name
