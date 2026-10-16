// @alias @/*
import { tax } from '../tax';
import { money } from '../money';
import type { Clock } from '@/clock'; // expect: mixed-path-style
export { format } from '../format';

export function label(total: number, clock: Clock): string {
  return money(tax(total)) + clock.now();
}
