enum Status {
  Open = 'open',
  Paid = 'paid',
  Void = 'void',
}

export function color(status: Status): string {
  if (status === Status.Open) { // expect: no-if-else-cascade
    return 'blue';
  } else if (status === Status.Paid) {
    return 'green';
  } else if (status === Status.Void) {
    return 'grey';
  }
  return 'black';
}

export function grams(unit: string): number {
  if (unit === 'kg') { // expect: no-if-else-cascade
    return 1000;
  }
  if (unit === 'g') {
    return 1;
  }
  if ('mg' === unit || 'mcg' === unit) {
    return 0.001;
  }
  return 0;
}

export function fee(order: { kind: string; total: number }): number {
  if (order.total > 100) {
    return 0;
  } else if (order.kind === 'gift') { // expect: no-if-else-cascade
    return 1;
  } else if (order.kind === 'bulk') {
    return 2;
  } else if (order.kind === 'rush') {
    return 3;
  }
  return 4;
}
