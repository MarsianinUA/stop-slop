export function invoke(operation: (...args: number[]) => number, args: number[]): number {
  return Reflect.apply(operation, undefined, args); // expect: no-reflect-apply
}

export function invokeComputed(operation: Function, owner: object, args: unknown[]): unknown {
  return Reflect['apply'](operation, owner, args); // expect: no-reflect-apply
}

export const invokeBound = (operation: Function, owner: object): unknown =>
  Reflect?.apply(operation, owner, []); // expect: no-reflect-apply

export function wrap<T extends (...args: never[]) => unknown>(operation: T): T {
  return new Proxy(operation, {
    apply(target, owner, args) {
      return Reflect.apply(target, owner, args); // expect: no-reflect-apply
    },
  });
}
