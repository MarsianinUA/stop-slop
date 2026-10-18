declare const flag: boolean;

export const isEnabled = true;
export const IS_STRICT = flag;
const _isDraft = !flag;
export const hasDraft = _isDraft;
const hasRows = (rows: string[]): boolean => rows.length > 0;

export function matches(left: string, right: string): boolean {
  return left === right;
}

export async function save(rows: string[]): Promise<boolean> {
  return hasRows(rows);
}

export function run({ dryRun = false }: { dryRun?: boolean }, canWrite: boolean | null): number {
  const shouldRetry = Boolean(canWrite) && !dryRun;
  let attempts = 0;
  const label = shouldRetry ? 'retry' : 'once';
  attempts += label.length;
  return attempts;
}

export class Job {
  enabled = true;
  protected paused = false;
  private isRunning = false;
  #hasStarted = false;

  start(): boolean {
    this.isRunning = !this.paused && this.enabled;
    this.#hasStarted = this.isRunning;
    return this.#hasStarted;
  }
}
