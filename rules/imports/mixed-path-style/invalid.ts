// @alias @/*
import { Clock } from '@/clock';
import { tax } from '../tax'; // expect: mixed-path-style
import { money } from '../money';
import { format } from './format';
import { config } from '@/config';

export function label(total: number): string {
  return format(money(tax(total)), Clock.now(), config.currency);
}
