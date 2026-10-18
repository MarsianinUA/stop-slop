import { Chain } from './chain';

enum Status {
  Open = 'open',
  Paid = 'paid',
}

type Shape = { kind: 'circle'; radius: number } | { kind: 'square'; side: number };

export function label(status: Status): string {
  switch (status) { // expect: exhaustive-switch
    case Status.Open:
      return 'open';
    case Status.Paid:
      return 'paid';
  }
}

export function area(shape: Shape): number {
  switch (shape.kind) { // expect: exhaustive-switch
    case 'circle':
      return Math.PI * shape.radius ** 2;
    case 'square':
      return shape.side ** 2;
  }
}

export function notes(status: Status, chain: Chain, shape?: Shape): string[] {
  const lines: string[] = [];
  switch (status) { // expect: exhaustive-switch
    case Status.Paid:
      lines.push('paid');
      break;
  }
  switch (chain) { // expect: exhaustive-switch
    case Chain.Ethereum:
    case Chain.Base:
      lines.push('evm');
      break;
  }
  switch (shape?.kind) { // expect: exhaustive-switch
    case 'circle':
      lines.push('round');
      break;
  }
  return lines;
}

const Mode = { Fast: 'fast', Slow: 'slow' } as const;
type Mode = (typeof Mode)[keyof typeof Mode];

export function delay(mode: Mode): number {
  switch (mode) { // expect: exhaustive-switch
    case Mode.Fast:
      return 10;
    case Mode.Slow:
      return 100;
  }
}
