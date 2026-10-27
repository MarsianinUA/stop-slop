interface Job {
  id: string;
}

declare const jobs: Job[];
declare const deque: { length: number; shift(): Job | undefined };
declare function run(job: Job): void;

export function other(queue: Job[], done: Job[]): void {
  while (queue.length) {
    done.shift();
    queue.pop();
  }
}

export function once(queue: Job[]): Job | undefined {
  return queue.shift();
}

export function unrelated(queue: Job[]): void {
  for (const job of jobs) {
    run(job);
    queue.shift();
  }
}

export function library(): void {
  while (deque.length) deque.shift();
}

export function later(queue: Job[]): void {
  while (queue.length) {
    setTimeout(() => queue.shift());
    queue.pop();
  }
}

export function indexed(queue: Job[]): void {
  for (let head = 0; head < queue.length; head++) run(queue[head]);
}
