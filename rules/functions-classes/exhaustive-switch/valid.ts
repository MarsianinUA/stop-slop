enum Status {
  Open = 'open',
  Paid = 'paid',
}

type Shape = { kind: 'circle'; radius: number } | { kind: 'square'; side: number };

export function label(status: Status): string {
  switch (status) {
    case Status.Open:
      return 'open';
    case Status.Paid:
      return 'paid';
    default: {
      const unhandled: never = status;
      return unhandled;
    }
  }
}

export function parse(raw: string): Status | null {
  switch (raw) {
    case Status.Open:
      return Status.Open;
    default:
      return null;
  }
}

export function area(shape: Shape): number {
  switch (shape.kind) {
    case 'circle':
      return Math.PI * shape.radius ** 2;
    default:
      throw new Error(`unknown shape ${shape.kind}`);
  }
}

export function code(key: string): number {
  switch (key) {
    case 'a':
      return 1;
    case 'b':
      return 2;
  }
  return 0;
}

export function width(value: unknown): number {
  switch (typeof value) {
    case 'string':
      return 1;
  }
  return 0;
}

export function size(shape: { type: 'small' | 'large' }): number {
  switch (shape.type) {
    case 'small':
      return 1;
  }
  return 2;
}

export function rank(status: Status): number {
  switch (status) {
    case Status.Open:
      return 1;
    case Status.Paid:
      return 2;
    default:
      throw new Error(`unknown status ${String(status)}`);
  }
}

export function step(key: string, keys: { up: string; down: string }): number {
  switch (key) {
    case keys.up:
      return 1;
    case keys.down:
      return -1;
  }
  return 0;
}
