// @path src/orders/orders.service.spec.ts
// @alias @/*
// @workspace @acme/db
import { OrdersService } from './orders.service';

jest.mock('./orders.repository'); // expect: no-internal-module-mock
jest.mock('../payments/payments.client', () => ({ charge: jest.fn() })); // expect: no-internal-module-mock
jest.mock('@/metrics', () => ({ // expect: no-internal-module-mock
  record: jest.fn(),
}));
jest.mock('@acme/db/client'); // expect: no-internal-module-mock
jest.mock('#db/client'); // expect: no-internal-module-mock
jest['doMock']('./clock'); // expect: no-internal-module-mock
jest.setMock('./orders.config.json', { retries: 3 }); // expect: no-internal-module-mock
jest.unstable_mockModule(`./mailer`, () => ({ send: jest.fn() })); // expect: no-internal-module-mock
vitest.doMock('./orders.audit'); // expect: no-internal-module-mock

describe('OrdersService', () => {
  beforeEach(() => {
    jest.doMock('./orders.cache'); // expect: no-internal-module-mock
  });

  it('places an order', () => {
    expect(new OrdersService()).toBeDefined();
  });
});
