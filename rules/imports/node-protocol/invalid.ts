import fs from 'fs'; // expect: node-protocol
import { readFile } from "fs/promises"; // expect: node-protocol
import * as path from 'path'; // expect: node-protocol
import type { Stats } from 'fs'; // expect: node-protocol
import 'process'; // expect: node-protocol
import {
  createHash,
  randomUUID,
} from 'crypto'; // expect: node-protocol
export { EventEmitter } from 'events'; // expect: node-protocol
export * from 'util'; // expect: node-protocol
export type { Server } from 'http'; // expect: node-protocol
import os = require('os'); // expect: node-protocol
const childProcess = require('child_process'); // expect: node-protocol
type Url = typeof import('url'); // expect: node-protocol
import assert from 'assert/strict'; // expect: node-protocol

export async function load() {
  const zlib = await import('zlib'); // expect: node-protocol
  return { zlib, fs, readFile, path, os, childProcess, createHash, randomUUID, assert };
}

export type Both = Stats | Url;
