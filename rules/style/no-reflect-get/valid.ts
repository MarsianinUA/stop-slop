interface Owner {
  id: string;
  name: string;
}

export function read(owner: Owner, key: keyof Owner): string {
  return owner[key];
}

export function readName(owner: Owner): string {
  return owner.name;
}

export function rename(owner: Owner, name: string): boolean {
  return Reflect.set(owner, 'name', name);
}

export function keys(owner: Owner): (string | symbol)[] {
  return Reflect.ownKeys(owner);
}

export function local(): number {
  const Reflect = { get: (): number => 1 };
  return Reflect.get();
}

export function parameter(Reflect: { get(): number }): number {
  return Reflect.get();
}

export function invoke(operation: (value: number) => number): number {
  return Reflect.apply(operation, undefined, [1]);
}
