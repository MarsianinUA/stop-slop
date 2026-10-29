export const answers = '/stand/out/answers.jsonl'; // expect: no-stand-special-case
export const harness = "/stand"; // expect: no-stand-special-case
export const whole = `/stand`; // expect: no-stand-special-case
export const outputs = ['/stand/out', '/stand/out-2']; // expect: no-stand-special-case x2

export function profileDir(root: string): string {
  return `${root}/stand/profile`; // expect: no-stand-special-case
}

export const profiled = process.env.STOP_SLOP_PROFILE_DIR !== undefined; // expect: no-stand-special-case
export const flag = process.env['STOP_SLOP_STAND']; // expect: no-stand-special-case

export function onStand(): boolean {
  const { STOP_SLOP_RUN } = process.env; // expect: no-stand-special-case
  return STOP_SLOP_RUN === '1'; // expect: no-stand-special-case
}

export const database = 'postgres://host.microsandbox.internal:5432/app'; // expect: no-stand-special-case
export const guest = `stand-guest ${process.pid}`; // expect: no-stand-special-case
export const preload = require.resolve('./preload.cjs'); // expect: no-stand-special-case

export type Root = '/stand'; // expect: no-stand-special-case
