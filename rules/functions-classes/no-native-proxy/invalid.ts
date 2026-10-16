export function traced<T extends object>(target: T, log: (key: string) => void): T {
  return new Proxy(target, { // expect: no-native-proxy
    get(object, key, receiver) {
      log(String(key));
      return Reflect.get(object, key, receiver);
    },
  });
}

export function revocable<T extends object>(target: T): { proxy: T; revoke: () => void } {
  return Proxy.revocable(target, {}); // expect: no-native-proxy
}

export const frozen = new Proxy({}, {}); // expect: no-native-proxy
