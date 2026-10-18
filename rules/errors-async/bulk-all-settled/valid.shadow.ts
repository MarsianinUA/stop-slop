import { Promise } from './bluebird';

declare function save(id: string): void;

export function storeAll(ids: string[]): unknown {
  return Promise.all(ids.map((id) => save(id)));
}
