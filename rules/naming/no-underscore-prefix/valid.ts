import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

type Handler = (request: unknown, response: { end(): void }) => void;

export const handle: Handler = (_request, response) => response.end();

export function pick<T>(items: T[]): T[] {
  return items.map((item, _) => item);
}

export class Store {
  protected _version = 1;
  #items: string[] = [];

  size(): number {
    return this.#items.length + this._version;
  }
}

export const root = __dirname;
