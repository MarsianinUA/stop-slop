// @path src/orders/orders.spec.ts
function makeRows(): string[] {
  return ['a'];
}

export function rows(): number {
  const made = makeRows();
  return made.length;
}
