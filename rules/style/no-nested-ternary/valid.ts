declare const count: number;
declare const flag: boolean;
declare const rows: number[] | undefined;

export const size = count > 10 ? 'large' : 'small';
export const pair = [flag ? 1 : 2, count > 0 ? 'some' : 'none'];
export const marks = rows ? rows.map((row) => (row > 0 ? '+' : '-')) : [];
export const label = flag
  ? class {
      kind = count > 1 ? 'many' : 'one';
    }
  : undefined;

export function sign(value: number): string {
  if (value > 0) {
    return 'positive';
  }
  return value < 0 ? 'negative' : 'zero';
}
