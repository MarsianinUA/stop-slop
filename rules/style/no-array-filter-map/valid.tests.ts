// @path src/orders/__tests__/order-routes.spec.ts
const ROUTES = [
  { path: '/orders', isPublic: true },
  { path: '/admin', isPublic: false },
];

describe('routes', () => {
  it('lists the public paths', () => {
    expect(ROUTES.filter((route) => route.isPublic).map((route) => route.path)).toEqual(['/orders']);
  });
});
