// @path src/orders/orders.service.spec.ts
const caseName = 'adds up the order lines';
const suffix = ' with a discount';

describe('works', () => {
  it('returns the sum of the order lines', () => {
    expect(1 + 1).toBe(2);
  });

  it('works offline', () => {
    expect(2 + 2).toBe(4);
  });

  test('rejects an empty order', () => {
    expect(3 + 3).toBe(6);
  });

  it('404', () => {
    expect(4 + 4).toBe(8);
  });

  it('ok', () => {
    expect(5 + 5).toBe(10);
  });

  it('case', () => {
    expect(6 + 6).toBe(12);
  });

  it.each([['EUR']])('test %s', (currency) => {
    expect(currency).toHaveLength(3);
  });

  test.each`
    amount | expected
    ${1}   | ${1}
  `('works as $expected', ({ amount, expected }) => {
    expect(amount).toBe(expected);
  });

  it(`works for ${'EUR'}`, () => {
    expect(7 + 7).toBe(14);
  });

  it(caseName, () => {
    expect(8 + 8).toBe(16);
  });

  it('works' + suffix, () => {
    expect(9 + 9).toBe(18);
  });

  test.describe('test', () => {
    test.beforeEach(() => undefined);
  });

  describe.each([[1]])('works 2', (value) => {
    it('keeps the value', () => {
      expect(value).toBe(1);
    });
  });
});
