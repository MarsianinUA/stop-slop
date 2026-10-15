type Payload = Record<string, unknown>; // expect: no-unsafe-dictionary-type
type Loose = { [key: string]: any }; // expect: no-unsafe-dictionary-type
type Mapped = { [K in string]: object }; // expect: no-unsafe-dictionary-type
type Empty = Record<string, {}>; // expect: no-unsafe-dictionary-type
type Mixed = Record<string, string | unknown>; // expect: no-unsafe-dictionary-type
type Frozen = Readonly<Record<string, unknown>>; // expect: no-unsafe-dictionary-type
type Wrapped = Record<string, Readonly<unknown>>; // expect: no-unsafe-dictionary-type
type Templated = Record<`id-${string}`, unknown>; // expect: no-unsafe-dictionary-type
type Suggested = Record<'a' | (string & {}), unknown>; // expect: no-unsafe-dictionary-type
type AnyKey = { [K in keyof any]: unknown }; // expect: no-unsafe-dictionary-type

interface Event {
  name: string;
  payload: Record<string, unknown>; // expect: no-unsafe-dictionary-type
  meta?: Record<string, unknown> | null; // expect: no-unsafe-dictionary-type
  [extra: string]: unknown; // expect: no-unsafe-dictionary-type
}

declare function Column(): PropertyDecorator;

class Store {
  private cache: Record<string, unknown> = {}; // expect: no-unsafe-dictionary-type

  @Column()
  data!: Record<string, unknown>; // expect: no-unsafe-dictionary-type
}

class Cache {
  constructor(
    private readonly entries: Record<string, unknown>, // expect: no-unsafe-dictionary-type
    readonly extra: Record<string, unknown>, // expect: no-unsafe-dictionary-type
  ) {}
}

interface Props extends Record<string, unknown> { // expect: no-unsafe-dictionary-type
  id: string;
}

class Bag implements Record<string, unknown> { // expect: no-unsafe-dictionary-type
  [key: string]: unknown; // expect: no-unsafe-dictionary-type
}

function toRow(): Record<string, unknown> { // expect: no-unsafe-dictionary-type
  const row: Record<string, unknown> = {}; // expect: no-unsafe-dictionary-type
  return row;
}

const rows: Array<Record<number, unknown>> = []; // expect: no-unsafe-dictionary-type

export type { Payload, Loose, Mapped, Empty, Mixed, Frozen, Wrapped, Templated, Suggested, AnyKey, Event, Props };
export { Store, Cache, Bag, toRow, rows };
