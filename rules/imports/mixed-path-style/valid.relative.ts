import { tax } from '../tax';
import { money } from '../../money';
import { format } from './format';
import { Clock } from '@acme/clock';

export function label(total: number): string {
  return format(money(tax(total)), Clock.now());
}
