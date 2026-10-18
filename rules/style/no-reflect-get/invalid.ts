export function read(owner: object, key: string): unknown {
  return Reflect.get(owner, key); // expect: no-reflect-get
}

export function readComputed(owner: object, key: PropertyKey): unknown {
  return Reflect['get'](owner, key); // expect: no-reflect-get
}

export const readId = (owner: object): unknown => Reflect?.get(owner, 'id'); // expect: no-reflect-get

export function traced<T extends object>(target: T, log: (key: PropertyKey) => void): T {
  return new Proxy(target, {
    get(object, key, receiver) {
      log(key);
      return Reflect.get(object, key, receiver); // expect: no-reflect-get
    },
  });
}

export function readAll(owner: object, keys: string[]): unknown[] {
  return keys.map((key) =>
    Reflect.get( // expect: no-reflect-get
      owner,
      key,
    ),
  );
}
