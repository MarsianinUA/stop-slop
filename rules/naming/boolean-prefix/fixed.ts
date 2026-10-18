declare function load(): Promise<string[]>;

export const enabled = true;
const IS_CACHED = false;

export async function sync(force = false): Promise<number> {
  let isDone = false;
  const rows = await load();
  const isEmpty = rows.length === 0;
  const isSettled: boolean = !isEmpty && rows.some((row) => row !== '');
  const nameChanged = rows[0] !== rows[1];
  const isLoaded = nameChanged;
  isDone = isSettled || force || IS_CACHED || isLoaded;
  return isDone ? rows.length : 0;
}

export function check(isNotReady: boolean): boolean {
  const ready = !isNotReady;
  return ready;
}

export function knownRow(value: string): value is 'row' {
  return value === 'row';
}

export type OnClose = (clean: boolean) => void;

export class Job {
  private running = false;
  #paused = false;

  private get stale(): boolean {
    return this.running && this.#paused;
  }

  tick(): boolean {
    this.running = !this.stale;
    this.#paused = !this.running;
    return this.#paused;
  }
}

export function pick(limit: number): boolean {
  const Ready = limit > 1;
  const inner = (): boolean => {
    const ready = limit > 2;
    return ready && Ready;
  };
  return inner();
}

export function both(isOn: boolean): boolean {
  const first = isOn;
  const second = isOn;
  const joined = first && second;
  return joined;
}

export function kinds(input: unknown): boolean {
  const list = Array.isArray(input);
  const whole = Number.isInteger(input);
  return list && whole;
}
