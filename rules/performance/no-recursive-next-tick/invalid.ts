declare const queue: string[];

export function drain(): void {
  queue.shift();
  if (queue.length > 0) process.nextTick(drain); // expect: no-recursive-next-tick
}

export const spin = (): void => {
  process.nextTick(spin); // expect: no-recursive-next-tick
};

export const wrapped = function loop(): void {
  process.nextTick(() => loop()); // expect: no-recursive-next-tick
};

export function retry(): void {
  process.nextTick(function () { // expect: no-recursive-next-tick
    retry();
  });
}

export class Pump {
  private pending = 3;

  run(): void {
    this.pending -= 1;
    if (this.pending > 0) process.nextTick(() => this.run()); // expect: no-recursive-next-tick
  }

  step(): void {
    process.nextTick(this.step.bind(this)); // expect: no-recursive-next-tick
  }

  tick = (): void => {
    process.nextTick(this.tick); // expect: no-recursive-next-tick
  };
}
