/// <reference types="node" />
/* eslint-disable no-console */

// The total includes the tax, rounded to cents.
// Note: refresh(options) runs once per minute.
// Retries (3)
// overwrites `species`
// total = price * count
// COMP = $50
// return early
// break
// e.g. options.timeout
// public API
// Note: important,
// - refresh(options)

/**
 * Adds the tax to a price.
 *
 * @example
 * withTax(10, 0.2); // 12
 */
export function withTax(price: number, rate: number): number {
  // eslint-disable-next-line no-console
  console.log(price);
  // prettier-ignore
  const factors = [1, 0, 0,
    0, 1, 0];
  // @ts-expect-error the factors are numbers, not strings
  const label: string = factors[0];
  // Usage:
  // ```ts
  // withTax(10, 0.2);
  // ```
  return price * (1 + rate) + label.length;
}

// TODO(denis): call refresh() from a scheduler
export function refresh(options: { timeout: number }): number {
  return options.timeout;
}

/* istanbul ignore next */
export const fallback = refresh({ timeout: 1 });
