import fs from 'node:fs';
import { readFile } from 'node:fs/promises';
import express from 'express';
import extra from 'fs-extra';
import local from './fs';
import polyfill from 'buffer/';
import test from 'test';
import type { Stats } from 'node:fs';

declare module 'fs' {
  export function extra(): void;
}

const name = 'fs';

function withLocalRequire(require: (id: string) => unknown) {
  return require('fs');
}

jest.mock('fs');

export { fs, readFile, express, extra, local, polyfill, test, name, withLocalRequire };
export type { Stats };
