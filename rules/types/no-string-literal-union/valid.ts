enum Field {
  Sku = 'sku',
  Price = 'price',
}

const SORT_ORDERS = ['asc', 'desc'] as const;
type SortOrder = (typeof SORT_ORDERS)[number];
type Status = 'open' | null;
type Code = 'a' | 1;
type Level = 1 | 2 | 3;
type Single = 'only';
type Prefixed = `id-${string}`;
type Keys = keyof { a: 1; b: 2 };
type Payment = { kind: 'card' } | { kind: 'invoice' };
interface Row { phase: 'backfill' | 'live' }

function pick(row: Row, mode: 'peek' | 'charge'): Pick<Row, 'phase'> {
  void mode;
  return row;
}

export { Field, SORT_ORDERS, pick };
export type { SortOrder, Status, Code, Level, Single, Prefixed, Keys, Payment, Row };
