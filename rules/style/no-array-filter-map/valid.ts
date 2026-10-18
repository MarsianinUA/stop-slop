interface User {
  active: boolean;
  email: string;
}

declare function active(user: User): boolean;
declare function email(user: User): string;
declare function fetchUsers(): User[];
declare const query: { filter(by: unknown): { map(to: unknown): string[] } };

export function lazy(users: User[]): string[] {
  return users.values().filter(active).map(email).toArray();
}

export function fromIterator(users: Iterable<User>): string[] {
  return Iterator.from(users).filter(active).map(email).toArray();
}

export function onePass(users: User[]): string[] {
  return users.flatMap((user) => (user.active ? [user.email] : []));
}

export function apart(users: User[]): [string[], User[]] {
  return [users.map(email), users.filter(active)];
}

export function twoMaps(users: User[]): string[] {
  return users.map(email).map((value) => value.trim());
}

export function twoFilters(users: User[]): User[] {
  return users.filter(active).filter((user) => user.email !== '');
}

export function builder(): string[] {
  return query.filter(active).map(email);
}

export function untyped(rows: { filter(by: unknown): { map(to: unknown): string[] } }): string[] {
  return rows.filter(active).map(email);
}

export function fetched(): string[] {
  const users = fetchUsers();
  return users.filter(active).map(email);
}

export function reassigned(users: User[], others: User[]): string[] {
  let current = users;
  current = others;
  return current.filter(active).map(email);
}

export function dynamic(users: User[], method: 'filter'): string[] {
  return users[method](active).map(email);
}

export function shadowed(
  Object: { keys(value: object): { filter(by: unknown): { map(to: unknown): string[] } } },
  value: object,
): string[] {
  return Object.keys(value).filter(Boolean).map(String);
}
