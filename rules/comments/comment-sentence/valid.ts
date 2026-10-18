// @dependency viem
/// <reference types="node" />
/* eslint-disable no-console */
import { z } from 'zod';
import { stringify } from 'yaml/util';

// The limit for one batch.
export const BATCH_LIMIT = 100;

// Is this the limit the partner agreed to?
//
// Yes: the contract says so!
export const PARTNER_LIMIT = 100;

// Retries the request once
// and then gives up.
export function fetchOnce(url: string): string {
  const timeout = 30; // Seconds, as the SDK reads them.
  return `${url}?timeout=${timeout}`;
}

// userId is the key of the cache.
//
// outboxSeq is a decimal string, so it survives JSON.
//
// node-redis retries on its own.
//
// null keeps the stored name.
//
// viem keeps the URL out of the error message.
//
// zod parses the input before it reaches the cache.
export const cache = new Map<string, string>();
export const userIdSchema = z.string();

export function loadUser(userId: string): string | undefined {
  // eslint-disable-next-line no-console
  console.log(userId);
  // prettier-ignore
  const order = [1, 2,
    3];
  // @ts-expect-error the cache holds strings, not numbers
  const first: number = cache.get(userId);
  // TODO(denis): read the database when the cache misses
  // and keep the result
  // await store.load(userId);
  return cache.get(userId) ?? `${first}${order.length}`;
}

// #region Writes

// Example:
// jobs: [A, B]
// stages: [[A], [B]]
export function saveUser(userId: string, name: string): void {
  cache.set(userId, name);
}

// #endregion

/**
 * Saves a batch of users.
 *
 *   saved    users.saved.<id>
 *   failed   users.failed.<id>
 *
 * - one call per user
 * - stops at the first error
 *
 * | field | meaning   |
 * |-------|-----------|
 * | id    | the user  |
 *
 * ## Example
 *
 * ```ts
 * saveAll([['a', 'Ann']])
 * ```
 *
 * @param users pairs of id and name
 * @returns nothing
 */
export function saveAll(users: [string, string][]): void {
  users.forEach(([userId, name]) => saveUser(userId, /* overwrite */ name));
}

/**
 * Steps:
 *   1. read the file
 *   2. parse it
 */
export function readAll(): void {}

/** @internal */
export const internal = true;

// ------------------------------------------------------------
//
// Spec: https://example.com/rfc/cache
//
// (Not used by the API.)
//
// Use `saveAll` for more than one user, not `saveUser` in a loop (`map`).

export const LISTS = 2;

// The cache keeps two lists.
// - users by id
// - users by name
export const USER_LISTS = LISTS;

export class UserStore {
  // get reads the cache and never the store.
  get(userId: string): string | undefined {
    return cache.get(userId);
  }
}

// yaml writes the lists to disk.
export const dump = stringify;

export function ignoreErrors(): void {
  try {
    saveUser('a', 'b');
  } catch { /* ignore */ }
  [1].forEach(() => { /* noop */ });
}

// ------
// Helpers.
// ------

/*****
 * Section Title.
 *****/

/**
 * my-lib v1.2.3
 * @license MIT
 */

// Copyright 2024 Acme Inc
//
// Licensed under the MIT License

export function kindOf(kind: string): number {
  switch (kind) {
    case 'a':
      return 1;
    // no default
  }
  return 0; // NOSONAR
}

export class AuditStore extends UserStore {
  /** {@inheritDoc UserStore.get} */
  override get(userId: string): string | undefined {
    return super.get(userId);
  }
}

// キャッシュを読む。
export const CACHE_NOTE = 1;

// We wait for
// init()
// before reading the config.
export const READY = true;

// 2^53 - 1
export const MAX = 2 ** 53 - 1;
