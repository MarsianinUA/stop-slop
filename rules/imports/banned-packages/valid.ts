import { randomUUID } from 'node:crypto';
import { uniq } from 'lodash-es';
import axiosRetry from 'axios-retry';
import { lodash } from './lodash';
import { uuid } from '@acme/uuid';

jest.mock('axios');

export const PACKAGE = 'axios';

export async function load(url: string): Promise<unknown> {
  const response = await fetch(url);
  return [randomUUID(), uniq, axiosRetry, lodash, uuid, response];
}
