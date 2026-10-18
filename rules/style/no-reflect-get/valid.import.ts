import { Reflect } from './reflection';

export function read(owner: object, key: string): unknown {
  return Reflect.get(owner, key);
}
