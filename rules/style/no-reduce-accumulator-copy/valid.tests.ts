// @path src/orders/order.mapper.spec.ts
const ROWS = [
  { id: 'a', total: 1 },
  { id: 'b', total: 2 },
];

describe('order mapper', () => {
  it('keys the rows by id', () => {
    const byId = ROWS.reduce((acc, row) => ({ ...acc, [row.id]: row.total }), {});
    expect(byId).toEqual({ a: 1, b: 2 });
  });
});
