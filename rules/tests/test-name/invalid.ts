// @path src/orders/orders.service.spec.ts
describe('OrdersService', () => {
  it('works', () => { // expect: test-name
    expect(1 + 1).toBe(2);
  });

  test('test1', () => { // expect: test-name
    expect(2 + 2).toBe(4);
  });

  it('should work correctly', () => { // expect: test-name
    expect(3 + 3).toBe(6);
  });

  it.only('Works as expected.', () => { // expect: test-name
    expect(4 + 4).toBe(8);
  });

  test.skip(`is working`, () => { // expect: test-name
    expect(5 + 5).toBe(10);
  });

  it.concurrent.only('test case 2', async () => { // expect: test-name
    expect(6 + 6).toBe(12);
  });

  it.each([[1, 1]])('basic test', (value, sum) => { // expect: test-name
    expect(value).toBe(sum);
  });

  it.each([[2], [3]])('case %#', (value) => { // expect: test-name
    expect(value).toBeGreaterThan(1);
  });

  test.each`
    value | sum
    ${1}  | ${1}
  `('example 3', ({ value, sum }) => { // expect: test-name
    expect(value).toBe(sum);
  });

  xit('spec', () => { // expect: test-name
    expect(7 + 7).toBe(14);
  });

  it.todo('another test'); // expect: test-name

  test.failing('', () => { // expect: test-name
    expect(8 + 8).toBe(17);
  });
});
