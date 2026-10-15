type Field = 'sku' | 'price'; // expect: no-string-literal-union
export type SortOrder = 'asc' | 'desc'; // expect: no-string-literal-union
export type Phase = // expect: no-string-literal-union
  | 'backfill'
  | 'live'
  | 'paused';
type Mode = 'peek' | 'charge' | null; // expect: no-string-literal-union

function sort(field: Field, order: SortOrder, mode: Mode): string {
  type Direction = "up" | "down"; // expect: no-string-literal-union
  const direction: Direction = order === 'asc' ? 'up' : 'down';
  return `${field}:${direction}:${mode}`;
}

export { sort };
