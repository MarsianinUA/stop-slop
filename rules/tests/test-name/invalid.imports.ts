// @path src/orders/__tests__/orders.mapper.ts
import pw, { test as e2e } from '@playwright/test';
import run, { test as nodeTest } from 'node:test';
import { it as check, test } from 'vitest';
import { it as bunIt } from 'bun:test';
import { it as mochaIt } from 'mocha';

check('works fine', () => { // expect: test-name
  expect(toOrder({ id: 1 }).id).toBe(1);
});

test.skipIf(false)('test 4', () => { // expect: test-name
  expect(toOrder({ id: 2 }).id).toBe(2);
});

test.for([3])('testing', (id) => { // expect: test-name
  expect(toOrder({ id }).id).toBe(id);
});

nodeTest('should work', () => { // expect: test-name
  expect(toOrder({ id: 4 }).id).toBe(4);
});

e2e('test 5', async () => { // expect: test-name
  expect(toOrder({ id: 5 }).id).toBe(5);
});

e2e.fixme('simple test', async () => { // expect: test-name
  expect(toOrder({ id: 6 }).id).toBe(6);
});

run('should work properly', () => { // expect: test-name
  expect(toOrder({ id: 7 }).id).toBe(7);
});

pw.skip('test 8', async () => { // expect: test-name
  expect(toOrder({ id: 8 }).id).toBe(8);
});

bunIt('works ok', () => {}); // expect: test-name
mochaIt('test 9', () => {}); // expect: test-name
pw.fail('it works', async () => {}); // expect: test-name

function toOrder(row: { id: number }): { id: number } {
  return { id: row.id };
}
