// the limit for one batch // expect: comment-sentence
export const BATCH_LIMIT = 100;

// The timeout for one request // expect: comment-sentence
export const REQUEST_TIMEOUT = 30;

// retries once and gives up. // expect: comment-sentence
export const RETRIES = 1;

// Retries the request once // expect: comment-sentence
// and then gives up
export function fetchOnce(url: string): string {
  const timeout = REQUEST_TIMEOUT; // in seconds // expect: comment-sentence
  return `${url}?timeout=${timeout}`;
}

/** The id of the user */ // expect: comment-sentence
export type UserId = string;

/* Cached per request */ // expect: comment-sentence
export const cache = new Map<UserId, string>();

/**
 * Loads the user.
 *
 * returns undefined when the id is unknown // expect: comment-sentence
 */
export function loadUser(id: UserId): string | undefined {
  // Reads the cache first // expect: comment-sentence
  // TODO(denis): read the database when the cache misses
  return cache.get(id);
}

// ---------- Writes ---------- // expect: comment-sentence

// see https://example.com/cache for the eviction rules. // expect: comment-sentence
export function saveUser(id: UserId, name: string): void {
  // Writes the cache; the store follows; // expect: comment-sentence
  cache.set(id, name);
  // Disabled until the store is back // expect: comment-sentence
  // store.save(id, name);
}

// Fallback when the cache misses // expect: comment-sentence
//cache.clear();
export const FALLBACK = 'cache';

// Steps // expect: comment-sentence
// - read the cache
// - write the store
/** The name of the user */ export type UserName = string; // expect: comment-sentence

export function readUser(id: UserId): string | undefined {
  // get the user from the cache. // expect: comment-sentence
  return cache.get(id);
}

/**
 * Loads the user.
 *
 *  returns null when the id is not cached // expect: comment-sentence
 */
export function findUser(id: UserId): string | undefined {
  return cache.get(id);
}

/* The store of last resort.

   second paragraph without a period */ // expect: comment-sentence
export const STORE = 'memory';

//No space before this line.
//
// a second paragraph without a period // expect: comment-sentence
export const SECOND = 2;

// =====
// the cache. // expect: comment-sentence
export const BANNER = 3;

// @Get(':id')
// find() {}
//
// the old handler // expect: comment-sentence
export const OLD = 4;

//   Kept until the store is back // expect: comment-sentence
//store.clear();
export const KEPT = 5;
