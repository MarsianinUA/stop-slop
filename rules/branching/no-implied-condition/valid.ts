import { isEnabled } from './flags';

declare function save(id: string): void;
declare function refresh(): boolean;

interface Order {
  isPaid: boolean;
  items: string[];
}

export function rewritten(): void {
  let isReady = refresh();
  if (isReady) {
    isReady = refresh();
    if (isReady) save('ready');
  }
}

export function reassigned(user: string | null): void {
  if (user) {
    user = user.trim() || null;
    if (user) save(user);
  }
}

export function property(order: Order): void {
  if (order.isPaid) {
    order.isPaid = refresh();
    if (order.isPaid) save('paid');
  }
}

export class Job {
  private isRunning = false;

  run(): void {
    if (this.isRunning) {
      if (this.isRunning) save('running');
    }
  }
}

export function callback(user: string | null, order: Order): void {
  if (user) {
    order.items.forEach((item) => {
      if (user) save(item);
    });
  }
}

export function repeatedElseIf(isPaid: boolean, isFree: boolean): void {
  if (isPaid) {
    save('paid');
  } else if (isPaid) {
    save('again');
  } else if (isFree || isPaid) {
    save('free');
  }
}

export function coalesced(count: number | null): number {
  if (count) {
    return count ?? 0;
  }
  return 0;
}

export function otherValue(kind: string): void {
  if (kind === 'a') {
    if (kind === 'b') save('b');
  }
}

export function loose(size: number): void {
  if (size == 1) {
    if (size == 1) save('one');
  }
}

export function value(user: string | null, isAdmin: boolean): string | null {
  if (user) {
    const shown = isAdmin && user;
    return shown || null;
  }
  return null;
}

export function imported(): void {
  if (isEnabled) {
    if (isEnabled) save('on');
  }
}

export function both(isPaid: boolean, isShipped: boolean): void {
  if (isPaid && isShipped) {
    save('done');
  } else {
    if (isPaid) save('paid');
  }
}

export function shadowed(isPaid: boolean): (isPaid: boolean) => string {
  if (isPaid) {
    return (isPaid) => (isPaid ? 'paid' : 'open');
  }
  return () => 'open';
}

export function classMembers(user: string | null): unknown {
  if (user) {
    return class {
      name = user ? user.trim() : '';
      static {
        if (user) save(user);
      }
    };
  }
  return null;
}
