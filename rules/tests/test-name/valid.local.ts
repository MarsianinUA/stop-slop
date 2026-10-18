// @path src/orders/orders.service.spec.ts
import { it } from './harness';

function test(name: string, run: () => void): void {
  run();
}

const suite = { it: (name: string) => name };

it('works', () => undefined);
test('test1', () => undefined);
suite.it('works');
