#!/usr/bin/env node
import { check } from './check';

export function run(args: string[]): number {
  return check(args);
}

process.exitCode = run(process.argv.slice(2));
