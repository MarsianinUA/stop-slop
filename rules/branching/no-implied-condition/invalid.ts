declare function save(id: string): void;
declare function load(): { id: string; isActive: boolean } | null;

const isLive = Math.random() > 0.5;

export function nested(user: string | null): void {
  if (user) {
    if (user) { // expect: no-implied-condition
      save(user);
    }
  }
}

export function conjunct(isPaid: boolean, isShipped: boolean): void {
  if (isPaid && isShipped) {
    if (isPaid) save('paid'); // expect: no-implied-condition
  }
}

export function inner(isCached: boolean): boolean {
  return isLive && (isLive || isCached); // expect: no-implied-condition
}

export function either(isPaid: boolean, isFree: boolean): boolean {
  return isPaid || (isPaid && isFree); // expect: no-implied-condition
}

export function otherwise(kind: string): void {
  if (kind === 'a') {
    save('a');
  } else {
    if (kind === 'a') save('again'); // expect: no-implied-condition
  }
}

export function opposite(user: string | null): void {
  if (!user) {
    save('none');
  } else if (user) { // expect: no-implied-condition
    save(user);
  }
}

export function laterBranch(isPaid: boolean, isFree: boolean): void {
  if (isPaid) {
    save('paid');
  } else if (isFree) {
    if (!isPaid) save('free'); // expect: no-implied-condition
  }
}

export function unequal(kind: string): string {
  if (kind === 'a') {
    return kind !== 'a' ? 'other' : 'a'; // expect: no-implied-condition
  }
  return kind;
}

export function typed(value: unknown): number {
  if (typeof value === 'string') {
    return typeof value === 'string' ? value.length : 0; // expect: no-implied-condition
  }
  return 0;
}

export function present(id: string | null): void {
  if (id != null) {
    if (id == null) return; // expect: no-implied-condition
    save(id);
  }
}

export function destructured(): void {
  const { isActive } = load() ?? { isActive: false };
  if (isActive) {
    const label = isActive ? 'on' : 'off'; // expect: no-implied-condition
    save(label);
  }
}

export function looped(ids: string[]): void {
  for (const id of ids) {
    if (id) {
      while (id && ids.length > 0) ids.pop(); // expect: no-implied-condition
    }
  }
}
