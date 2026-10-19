interface Session {
  id: string;
  token?: string;
}

interface Index {
  [id: string]: number;
}

type Flags = Record<string, boolean>;

interface Bag extends Record<string, string> {}

export function forget(cache: Record<string, number>, key: string): void {
  delete cache[key];
  delete cache.total;
}

export function clear(flags: Flags, index: Index, names: { [key: string]: string }): void {
  delete flags.debug;
  delete index.first;
  delete names.admin;
}

export function extended(bag: Bag): void {
  delete bag.temp;
}

export function generic<T extends Record<string, unknown>>(value: T): void {
  delete value.secret;
}

export function copies(): void {
  const headers = {} as Record<string, string>;
  delete headers.host;
  const env = { ...process.env };
  delete env.DEBUG;
  const counts = { [String(Date.now())]: 1 };
  delete counts.total;
}

export function untyped(input: any, raw: unknown, settings): void {
  delete input.secret;
  delete (raw as { a?: number }).a;
  delete settings.verbose;
}

export function signedOut(session: Session): Session {
  return { ...session, token: undefined };
}

export function nested(holder: { session: Session }): void {
  delete holder.session.token;
}

export function unset(): void {
  delete process.env.DEBUG;
}

export function qualified(env: NodeJS.ProcessEnv): void {
  delete env.DEBUG;
}

export class Store {
  [key: string]: unknown;

  clear(): void {
    delete this.temp;
  }
}

export class Registry {
  static items?: string[];

  static reset(): void {
    delete this.items;
  }
}
