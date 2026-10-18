// @path src/orders/legacy.cts
// @alias @/*
import tax = require('../tax'); // expect: mixed-path-style
import { Clock } from '@/clock';
import { format } from '@/format';

export function label(total: number): string {
  return format(tax.apply(total), Clock.now());
}
