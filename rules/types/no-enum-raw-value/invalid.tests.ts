// @path src/sources/__tests__/source-sync.spec.ts
enum Source {
  Dashboard = 'DASHBOARD',
}

declare function expect(value: unknown): { toBe(expected: unknown): void };
declare const synced: { source: string };

expect(synced.source).toBe('DASHBOARD'); // expect: no-enum-raw-value
