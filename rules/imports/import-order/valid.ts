// @alias @/*
// @workspace @acme/shared
// @dependency events
import 'reflect-metadata';
import { readFile } from 'node:fs/promises';
import {} from './setup';
import fs from 'fs';
import type { Stats } from 'node:fs';
import { z } from 'zod';
import { EventEmitter } from 'events';
import { money } from '@acme/shared/money';
import type { Clock } from '@/clock';
import { round } from '@acme/shared/round';

import { tax } from './tax';
import type { Row } from '../row';
import './polyfill';
import { parse } from './parse';
export { format } from './format';
import { Module } from '@nestjs/common';
import './telemetry';
import { metrics } from './metrics';
import { trace } from './trace';

import { Injectable } from '@nestjs/common';

export class Orders extends EventEmitter {
  describe(row: Row, clock: Clock, stats: Stats): string {
    void [readFile, fs, Module, metrics, trace, Injectable];
    return money(tax(round(row.total))) + clock.now() + stats.size + parse(z.string().parse(row.id));
  }

  async lazy(): Promise<unknown> {
    const { draft } = await import('./draft');
    return draft;
  }
}
