// @path src/orders/orders.service.spec.ts
import { vi } from './test-utils';

vi.mock('./orders.repository');

function stubModules(jest: { mock(path: string): void }): void {
  jest.mock('./orders.cache');
}

const mock = { module: (path: string) => path };
mock.module('./clock');

stubModules({ mock: () => undefined });
