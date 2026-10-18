// @path src/orders/__tests__/orders.mapper.ts
import { test as e2e } from '@playwright/test';
import { test as nodeTest } from 'node:test';
import { it as check, test } from 'vitest';

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

function toOrder(row: { id: number }): { id: number } {
  return { id: row.id };
}
