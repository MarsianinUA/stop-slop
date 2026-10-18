import { Reflect } from './reflection';

export function invoke(operation: () => number): number {
  return Reflect.apply(operation, undefined, []);
}
