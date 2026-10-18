declare function load(): Promise<string[]>;

export const enabled = true; // expect: boolean-prefix
const CACHED = false; // expect: boolean-prefix

export async function sync(force = false): Promise<number> { // expect: boolean-prefix
  let done = false; // expect: boolean-prefix
  const rows = await load();
  const empty = rows.length === 0; // expect: boolean-prefix
  const settled: boolean = !empty && rows.some((row) => row !== ''); // expect: boolean-prefix
  const nameChanged = rows[0] !== rows[1]; // expect: boolean-prefix
  const loaded = nameChanged; // expect: boolean-prefix
  done = settled || force || CACHED || loaded;
  return done ? rows.length : 0;
}

export function check(isNotReady: boolean): boolean {
  const ready = !isNotReady; // expect: boolean-prefix
  return ready;
}

export function knownRow(value: string): value is 'row' { // expect: boolean-prefix
  return value === 'row';
}

export type OnClose = (clean: boolean) => void; // expect: boolean-prefix

export class Job {
  private running = false; // expect: boolean-prefix
  #paused = false; // expect: boolean-prefix

  private get stale(): boolean { // expect: boolean-prefix
    return this.running && this.#paused;
  }

  tick(): boolean {
    this.running = !this.stale;
    this.#paused = !this.running;
    return this.#paused;
  }
}

export function pick(limit: number): boolean {
  const Ready = limit > 1; // expect: boolean-prefix
  const inner = (): boolean => {
    const ready = limit > 2; // expect: boolean-prefix
    return ready && Ready;
  };
  return inner();
}

export function both(isOn: boolean): boolean {
  const first = isOn; // expect: boolean-prefix
  const second = isOn; // expect: boolean-prefix
  const joined = first && second; // expect: boolean-prefix
  return joined;
}

export function kinds(input: unknown): boolean {
  const list = Array.isArray(input); // expect: boolean-prefix
  const whole = Number.isInteger(input); // expect: boolean-prefix
  return list && whole;
}
