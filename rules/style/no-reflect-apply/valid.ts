export function invoke(operation: (...args: number[]) => number, args: number[]): number {
  return operation(...args);
}

export function invokeOn(operation: (this: object, value: number) => number, owner: object): number {
  return operation.apply(owner, [1]);
}

export function read(owner: { id: string }): unknown {
  return Reflect.get(owner, 'id');
}

export function local(): number {
  const Reflect = { apply: (): number => 1 };
  return Reflect.apply();
}

export function parameter(Reflect: { apply(): number }): number {
  return Reflect.apply();
}
