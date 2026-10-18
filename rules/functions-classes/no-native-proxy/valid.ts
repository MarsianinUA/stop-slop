import { Proxy as HttpProxy } from './http-proxy';

export interface Store {
  read(key: string): string | undefined;
}

export class TracedStore implements Store {
  constructor(
    private readonly store: Store,
    private readonly log: (key: string) => void,
  ) {}

  read(key: string): string | undefined {
    this.log(key);
    return this.store.read(key);
  }
}

export const http = new HttpProxy('http://localhost');

export function local(): unknown {
  class Proxy {
    constructor(readonly target: object) {}
  }
  return new Proxy({});
}
