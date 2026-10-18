export function transfer(from: string, to: string, amount: number): string { // expect: max-positional-params
  return `${from}->${to}:${amount}`;
}

export function pick(first: string, second: string, third: string): string;
export function pick(first: number, second: number, third: number): number;
export function pick(first: unknown, second: unknown, third: unknown): unknown { // expect: max-positional-params
  return first ?? second ?? third;
}

export const scale = (value: number, factor: number, digits = 2): number => // expect: max-positional-params
  Number((value * factor).toFixed(digits));

function join(separator: string, first: string, ...rest: string[]): string { // expect: max-positional-params
  return [first, ...rest].join(separator);
}

export class Ledger {
  private readonly format = (account: string, amount: number, currency: string): string => // expect: max-positional-params
    `${account}:${amount} ${currency}`;

  record(account: string, [amount, currency]: [number, string], note?: string): string { // expect: max-positional-params
    return join(' ', this.format(account, amount, currency), note ?? '');
  }
}

export class LedgerError extends Error {
  constructor(account: string, amount: number, reason: string) { // expect: max-positional-params
    super(`${account} ${amount}: ${reason}`);
  }
}
