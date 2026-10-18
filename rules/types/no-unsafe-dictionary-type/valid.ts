type Command = () => void;
type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

type Commands = Record<string, Command>;
type Levels = Record<'admin' | 'reader', number>;
type Indexed = { [key: string]: Command };
type Exhaustive = { [K in 'a' | 'b']: number };
type Nested = Record<string, { payload: unknown }>;
type Lookup = Map<string, unknown>;
type Json = Record<string, JsonValue>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function parseRow(raw: Record<string, unknown>): { id: string } {
  return { id: String(raw.id) };
}

function keys<T extends Record<string, unknown>>(input: T): string[] {
  return Object.keys(input);
}

declare const body: unknown;
const fields = Object.keys(body as Record<string, unknown>);

interface Handler {
  handle(event: Record<string, unknown>): void;
}

type Callback = (input: Record<string, unknown>) => void;

type IsRecord<T> = T extends Record<string, unknown> ? true : false;

const defaults = { retries: 3 } satisfies Record<string, unknown>;

class Parser {
  constructor(raw: Record<string, unknown>) {
    void raw;
  }
}

interface Labels extends Record<string, string> {
  title: string;
}

function local(): unknown {
  type Record<K extends string, V> = { key: K; value: V };
  type Pair = Record<'a', unknown>;
  const pair: Pair = { key: 'a', value: 1 };
  return pair;
}

export type { Commands, Levels, Indexed, Exhaustive, Nested, Lookup, Json, Handler, Callback, IsRecord, Labels };
export { isRecord, parseRow, keys, fields, defaults, Parser, local };
