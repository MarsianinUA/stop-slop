import type { Remote } from './remote';

interface Row {
  discount: number | null;
  note?: string;
}

export function price(row: Row): number {
  return row.discount ?? 0;
}

export function size(options: Remote): number {
  return options.size ?? 1;
}

export function notes(rows: Row[]): string[] {
  return rows.map((row: Row) => row.note ?? '');
}

export function retry({ retries = 3 }: { retries?: number } = {}): number {
  return retries;
}

export function clamp(value: number, limit?: number): number {
  return Math.min(value, limit ?? 10);
}

export class Poller {
  readonly intervalMs: number;

  constructor(readonly options: { intervalMs?: number } = {}) {
    this.intervalMs = options.intervalMs ?? 1000;
  }
}
