import * as z from 'zod';

export const schema = z.string();

export function sum(rows: number[]): number {
  let total = 0;
  for (let i = 0; i < rows.length; i++) {
    total += rows[i];
  }
  return rows.map((x) => x * 2).filter(function (v) { return v > 0; }).length + total;
}

export function sorted(rows: number[]): number[] {
  return [...rows].sort((a, b) =>
    a - b,
  );
}

export function first<T>(items: T[]): T | undefined {
  return items[0];
}

export type Keys<T> = { [K in keyof T]: K };

export function pick({ a, b }: { a: number; b: number }): number {
  return a + b;
}

export function parse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch (e) {
    return e;
  }
}

export function ignore(_: string, $: number): number {
  return $;
}

declare function log(x: string): void;
export const logged = log;

export const handlers = {
  onPick: (e: Event) => e.type,
  onDrop: (e: Event) =>
    e.timeStamp,
};
