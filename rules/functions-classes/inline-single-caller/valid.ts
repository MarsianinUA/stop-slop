declare function audit(row: string): void;

function twice(): number {
  return 1;
}

export function pair(): number {
  return twice() + twice();
}

function isOrder(value: unknown): value is string {
  return typeof value === 'string';
}

export function check(value: unknown): string {
  const known = isOrder(value);
  return known ? String(value) : '';
}

function fee(count: number): number {
  return count > 10 ? 1 : 0;
}

export function total(count: number): number {
  return count + fee(count);
}

function onRow(row: string): void {
  audit(row);
}

export function each(rows: string[]): void {
  rows.forEach((row) => {
    onRow(row);
  });
}

function handler(): void {}

export const handlers = [handler];

export function exported(): number {
  return 1;
}

export function useExported(): number {
  return exported();
}

function* ids(): Generator<number> {
  yield 1;
}

export function first(): number {
  const [id] = ids();
  return id ?? 0;
}

function drain(items: string[]): void {
  if (items.length > 0) {
    items.pop();
    drain(items);
  }
}

export class Orders {
  readonly handler = this.bound.bind(this);

  run(): number {
    const step = this.step();
    return this.helper() + this.helper() + step;
  }

  private helper(): number {
    return 1;
  }

  protected step(): number {
    return 1;
  }

  private bound(): number {
    return 1;
  }
}

async function bootstrap(): Promise<void> {
  audit('start');
}

bootstrap();

function onClick(): void {
  audit('click');
}

export const buttons = {
  click: () => {
    onClick();
  },
};

function onSave(): void {
  audit('save');
}

export function saveHandlers(): Array<() => void> {
  const list = [onSave];
  return list;
}
