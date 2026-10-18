// @alias @/*
// @alias @acme/shared/*
// @workspace @acme/shared
import { z } from 'zod';
import { money } from '@acme/shared/money';
import { Clock } from '@/clock';
import { format } from './format';
import { parse } from './parsers/parse';

export async function label(total: number): Promise<string> {
  const { tax } = await import('../tax');
  return format(money(tax(total)), Clock.now()) + parse(z.string().parse('x'));
}
