import './telemetry';

import { exec } from 'node:child_process';
import { promisify } from 'node:util';

import { createLogger, defineConfig, registerHandlers } from './startup';

export const logger = createLogger('orders');

const execAsync = promisify(exec);

export function start(): Promise<unknown> {
  registerHandlers();
  return execAsync('true');
}

export class Orders {
  static readonly NAME = 'orders';
}

export type OrderId = string;

export enum Status {
  Open = 'open',
}

declare global {
  interface Window {
    orders: Orders;
  }
}

export default defineConfig({ entry: 'src/main.ts' });
