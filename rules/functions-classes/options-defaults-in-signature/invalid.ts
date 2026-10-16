export interface RetryOptions {
  retries?: number;
  delayMs?: number;
  label: string | null;
}

export type ClientOptions = {
  timeoutMs?: number;
};

export function retry(task: () => void, options: RetryOptions): number {
  const retries = options.retries ?? 3; // expect: options-defaults-in-signature
  const label = options.label ?? 'task';
  for (let attempt = 0; attempt < retries; attempt += 1) {
    task();
  }
  return label.length + (options.delayMs || 100); // expect: options-defaults-in-signature
}

export function connect(url: string, config?: { timeoutMs?: number }): string {
  return `${url}:${config?.timeoutMs ?? 1000}`; // expect: options-defaults-in-signature
}

export function page(input: Partial<{ limit: number; cursor: string }>): string {
  return `${input.limit ?? 50}:${input.cursor ?? ''}`; // expect: options-defaults-in-signature x2
}

export function sorted(rows: string[], options: { order?: 'asc' | 'desc' } = {}): string[] {
  const { order = 'asc' } = options; // expect: options-defaults-in-signature
  return order === 'asc' ? [...rows].sort() : [...rows].sort().reverse();
}

export class Client {
  readonly timeoutMs: number;

  constructor(options: ClientOptions = {}) {
    this.timeoutMs = options.timeoutMs ?? 30_000; // expect: options-defaults-in-signature
  }
}

export function backoff(attempt: number, options?: { baseMs?: number }): number {
  const { baseMs = 100 } = options ?? {}; // expect: options-defaults-in-signature
  return baseMs * 2 ** attempt;
}
