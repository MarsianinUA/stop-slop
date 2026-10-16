// @alias @/*
import { readFile } from 'node:fs/promises';
import { performance } from 'node:perf_hooks';
import { Injectable } from '@nestjs/common';

import { Clock } from '@/clock';
import { parse } from './parse';

@Injectable()
export class Loader {
  constructor(private readonly clock: Clock) {}

  async load(path: string): Promise<number> {
    const started = performance.now();
    parse(await readFile(path, 'utf8'), this.clock.now());
    return performance.now() - started;
  }
}
