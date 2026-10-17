declare function load(id: string): Promise<string>;
declare function release(): void;
declare const pending: Promise<number> | undefined;

export function title(id: string): Promise<string> {
  return load(id).then((text) => text.trim()); // expect: no-then-chain
}

export function size(id: string): Promise<number> {
  return load(id)
    .then((text) => text.length) // expect: no-then-chain
    .then((length) => length * 2); // expect: no-then-chain
}

export function track(task: Promise<void>): void {
  void task.then(release, release); // expect: no-then-chain
}

export function later(): Promise<number | undefined> | undefined {
  return pending?.then((count) => count + 1); // expect: no-then-chain
}

export class Queue {
  private tail: Promise<void> = Promise.resolve();

  add(job: () => Promise<void>): Promise<void> {
    this.tail = this.tail.then(job, job); // expect: no-then-chain
    return this.tail;
  }
}
