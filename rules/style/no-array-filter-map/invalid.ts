interface User {
  active: boolean;
  email: string | undefined;
}

declare function active(user: User): boolean;
declare function email(user: User): string | undefined;
declare function present(value: string | undefined): value is string;

export const doubled = [1, 2, 3].filter((n) => n > 1).map((n) => n * 2); // expect: no-array-filter-map

export function emails(users: User[]): (string | undefined)[] {
  return users.filter(active).map(email); // expect: no-array-filter-map
}

export function known(users: readonly User[]): string[] {
  return users.map(email).filter(present); // expect: no-array-filter-map
}

export function generic(users: ReadonlyArray<User>): (string | undefined)[] {
  return users.filter(active).map((user) => user.email); // expect: no-array-filter-map
}

export function lib(users: Array<User>): string[] {
  return users.map((user) => user.email ?? '').filter(Boolean); // expect: no-array-filter-map
}

export function aliased(users: User[]): (string | undefined)[] {
  const all = users;
  return all.filter((user) => user.active).map(email); // expect: no-array-filter-map
}

export function optional(users: User[] | undefined): (string | undefined)[] | undefined {
  return users?.filter(active)?.map(email); // expect: no-array-filter-map
}

export function computed(users: User[]): (string | undefined)[] {
  return users['filter'](active)['map'](email); // expect: no-array-filter-map
}

export function asserted(users: User[]): (string | undefined)[] {
  return (users.filter(active)!).map((user) => email(user)); // expect: no-array-filter-map
}

export function copied(users: User[]): (string | undefined)[] {
  return users.slice().filter(active).map(email); // expect: no-array-filter-map
}

export function both(users: User[]): (string | undefined)[] {
  return users.filter(active).map(email).filter(Boolean); // expect: no-array-filter-map x2
}

export function entries(prices: { [name: string]: number }): string[] {
  return Object.entries(prices).filter(([, price]) => price > 0).map(([name]) => name); // expect: no-array-filter-map
}

export function unique(ids: Set<string>): string[] {
  return Array.from(ids) // expect: no-array-filter-map
    .filter((id) => id !== '')
    .map((id) => id.trim());
}

const LIMITS = [10, 20, 30];

export const raised = LIMITS.map((limit) => limit * 2).filter((limit) => limit > 30); // expect: no-array-filter-map
