import dayjs from 'dayjs';
import utc from 'dayjs/plugin/utc';

import { registerHandlers, warmUp } from './startup';

dayjs.extend(utc); // expect: no-top-level-side-effect
process.env.TZ ??= 'UTC'; // expect: no-top-level-side-effect
Error.stackTraceLimit = 50; // expect: no-top-level-side-effect

export function format(date: Date): string {
  return dayjs(date).utc().format();
}

if (process.env.NODE_ENV !== 'production') { // expect: no-top-level-side-effect
  Error.stackTraceLimit = 100;
}

for (const signal of ['SIGINT', 'SIGTERM'] as const) { // expect: no-top-level-side-effect
  process.on(signal, () => undefined);
}

{
  registerHandlers(); // expect: no-top-level-side-effect
}

void (async () => { // expect: no-top-level-side-effect
  await warmUp();
})();

await warmUp(); // expect: no-top-level-side-effect
