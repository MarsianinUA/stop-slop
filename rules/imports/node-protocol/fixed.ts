import fs from 'node:fs';
import { readFile } from "node:fs/promises";
import * as path from 'node:path';
import type { Stats } from 'node:fs';
import 'node:process';
import {
  createHash,
  randomUUID,
} from 'node:crypto';
export { EventEmitter } from 'node:events';
export * from 'node:util';
export type { Server } from 'node:http';
import os = require('node:os');
const childProcess = require('node:child_process');
type Url = typeof import('node:url');
import assert from 'node:assert/strict';

export async function load() {
  const zlib = await import('node:zlib');
  return { zlib, fs, readFile, path, os, childProcess, createHash, randomUUID, assert };
}

export type Both = Stats | Url;
