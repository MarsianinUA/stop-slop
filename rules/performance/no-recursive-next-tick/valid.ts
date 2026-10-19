declare const queue: string[];
declare const emitter: { on(event: string, listener: () => void): void };
declare function read(done: () => void): void;

export function drain(): void {
  queue.shift();
  if (queue.length > 0) setImmediate(drain);
}

export function flush(): void {
  process.nextTick(report);
}

function report(): void {
  queue.length = 0;
}

export function poll(): void {
  read(() => process.nextTick(poll));
}

export function listen(): void {
  emitter.on('data', () => process.nextTick(listen));
}

export class Pump {
  run(): void {
    process.nextTick(() => this.stop());
  }

  stop(): void {
    queue.length = 0;
  }
}

export function scheduled(process: { nextTick(callback: () => void): void }): void {
  process.nextTick(() => scheduled(process));
}
