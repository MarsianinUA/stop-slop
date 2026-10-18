// @path src/orders/__tests__/orders.mapper.ts
import { jest as runner } from '@jest/globals';
import { mock } from 'bun:test';
import { mock as nodeMock } from 'node:test';
import { vi as testApi } from 'vitest';

runner.mock('./orders.repository'); // expect: no-internal-module-mock
testApi.mock('./orders.mapper.helper'); // expect: no-internal-module-mock
testApi.doMock(import('./orders.cache'), () => ({ get: testApi.fn() })); // expect: no-internal-module-mock
vi.mock('../shared/clock'); // expect: no-internal-module-mock
mock.module('./orders.client', () => ({ send: () => undefined })); // expect: no-internal-module-mock
nodeMock.module('./orders.config.js', { namedExports: { retries: 3 } }); // expect: no-internal-module-mock
