// @alias @/*
// @workspace @acme/shared
// @dependency events
import 'reflect-metadata';
import type { OrderRow } from './order.row'; // expect: import-order
import { readFile } from 'node:fs/promises';
import { EventEmitter } from 'events';

import { Injectable } from '@nestjs/common';

import { money } from '@acme/shared/money';
import type { Clock } from '@/clock';
import { tax } from './tax';
import './polyfill';
import { format } from './format'; // expect: import-order
// Pino types for the logger below.
import type { Logger } from 'pino';
import type { Audit } from '@/audit'; // the audit trail
import './register';
import { z } from 'zod';

// Local types
import type { Line } from './line';
import type { Ledger } from '@acme/shared/ledger'; // expect: import-order
import './metrics';
import { parse } from './parse';
import { Module } from '@nestjs/common'; // expect: import-order
import './telemetry';
import { round } from '@acme/shared/round';
import { Inject } from '@nestjs/common'; // expect: import-order
import './tracing';
// Rows as the database returns them.
import type { Row } from './row';
import { Controller } from '@nestjs/common'; // expect: import-order
import './bootstrap';
import type { Cart } from './cart';
// Shared

import type { Price } from '@acme/shared/price'; // expect: import-order
import './env';
import type { Draft } from './draft';
/* eslint-disable no-restricted-imports */
import type { Tax } from '@acme/shared/tax'; // expect: import-order
/* eslint-enable no-restricted-imports */
import './final';
import type { Fee } from './fee'; // eslint-disable-next-line no-restricted-imports
import type { Coupon } from '@acme/shared/coupon'; // expect: import-order

export class Orders extends EventEmitter {
  describe(row: OrderRow, clock: Clock, logger: Logger, audit: Audit, line: Line, ledger: Ledger): string {
    void readFile;
    void [Injectable, Module, Inject, Controller];
    logger.info(audit.id + line.id + ledger.id);
    return format(money(tax(round(row.total)))) + clock.now() + parse(z.string().parse(row.id));
  }

  price(row: Row, cart: Cart, price: Price, draft: Draft, taxed: Tax, fee: Fee, coupon: Coupon): number {
    return row.total + cart.total + price.total + draft.total + taxed.total + fee.total + coupon.total;
  }
}
