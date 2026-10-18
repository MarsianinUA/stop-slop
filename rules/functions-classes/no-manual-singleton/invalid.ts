export class Cache {
  private static instance: Cache | undefined; // expect: no-manual-singleton

  private constructor(private readonly entries: Map<string, string>) {}

  static getInstance(): Cache {
    if (!Cache.instance) {
      Cache.instance = new Cache(new Map());
    }
    return Cache.instance;
  }

  read(key: string): string | undefined {
    return this.entries.get(key);
  }
}

export class Registry {
  static #shared?: Registry; // expect: no-manual-singleton

  static get shared(): Registry {
    return (this.#shared ??= new Registry());
  }
}

export class Clock {
  private static readonly current = new Clock(); // expect: no-manual-singleton

  static instance(): Clock {
    return Clock.current;
  }

  now(): number {
    return Date.now();
  }
}

export const Metrics = class Metrics {
  static active: Metrics | null = null; // expect: no-manual-singleton

  static use(): Metrics {
    Metrics.active ??= new Metrics();
    return Metrics.active;
  }
};
