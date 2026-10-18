export class OrderManager {} // expect: no-vague-name

export interface BaseRepository { // expect: no-vague-name
  save(row: string): void;
}

export type CommonOptions = { retries: number }; // expect: no-vague-name

export function formatHelper(value: string): string { // expect: no-vague-name
  return value.trim();
}

export const parseUtils = (text: string): number => Number(text); // expect: no-vague-name

export function summary(rows: string[]): number {
  const data = rows.join(','); // expect: no-vague-name
  const result = data.length; // expect: no-vague-name
  let temp = result; // expect: no-vague-name
  temp += rows.length;
  return temp;
}

export function head(rows: string[]): string | undefined {
  const item = rows[0]; // expect: no-vague-name
  return item;
}

export class Orders {
  private buildHelper(): string { // expect: no-vague-name
    return 'orders';
  }

  label(): string {
    return this.buildHelper();
  }
}

export const sortUtil = function sortUtil(rows: string[]): string[] { // expect: no-vague-name
  return [...rows].sort();
};
