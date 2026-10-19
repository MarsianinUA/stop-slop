interface User {
  name: string;
  password?: string;
}

export class Cache {
  private entries?: Map<string, string>;

  reset(): void {
    delete this.entries; // expect: no-delete-property
  }
}

export function publicUser(user: User): User {
  delete user.password; // expect: no-delete-property
  return user;
}

export function draft(name: string, password: string): string {
  const user: { name: string; password?: string } = { name, password };
  delete user.password; // expect: no-delete-property
  return JSON.stringify(user);
}

export function quiet(): boolean {
  const options = { retries: 3, verbose: true } as { retries?: number; verbose: boolean };
  delete options.retries; // expect: no-delete-property
  return options.verbose;
}

export function copied(user: User): User {
  const copy = { ...user };
  delete copy.password; // expect: no-delete-property
  return copy;
}

export function cleared(patch: Partial<User> | null): void {
  if (patch) delete patch['password']; // expect: no-delete-property
}

export function cast(user: User): void {
  delete (user as any).name; // expect: no-delete-property
}
