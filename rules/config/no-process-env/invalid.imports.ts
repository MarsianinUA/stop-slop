import process from 'node:process';
import { default as nodeProcess, env } from 'node:process'; // expect: no-process-env
import * as proc from 'process';

export function host(): string | undefined {
  return process.env.HOST; // expect: no-process-env
}

export function user(): string | undefined {
  return proc.env.USER; // expect: no-process-env
}

export function zone(): string | undefined {
  return nodeProcess.env.TZ; // expect: no-process-env
}

export function home(): string | undefined {
  return env.HOME;
}
