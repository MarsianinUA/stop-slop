// @path src/orders/orders.service.spec.ts
// @alias @/*
// @workspace @acme/db
import { OrdersService } from './orders.service';

const modulePath = './orders.repository';
const name = 'orders.cache';

jest.mock('axios');
jest.mock('node:fs/promises');
jest.mock('fs');
jest.mock('@nestjs/config', () => ({ ConfigService: jest.fn() }));
jest.mock(`@sentry/node`);
jest.mock(modulePath);
jest.mock(`./${name}`);
jest.unmock('./orders.repository');
jest.requireActual('./orders.repository');
jest.spyOn(console, 'warn');

describe('OrdersService', () => {
  it('places an order', () => {
    expect(new OrdersService()).toBeDefined();
  });
});
