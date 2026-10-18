// @path src/orders.spec.ts
function row(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { id: 1, ...overrides };
}

export { row };
