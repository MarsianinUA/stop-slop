export function requestOptions(timeout: number | undefined, retries?: number): object {
  return {
    method: 'GET',
    ...(timeout !== undefined ? { timeout } : {}), // expect: no-conditional-empty-object-spread
    ...(retries ? {} : { retries: 3 }), // expect: no-conditional-empty-object-spread
  };
}

export const headers = (token?: string): object => ({
  accept: 'application/json',
  ...(token ? { authorization: `Bearer ${token}` } : ({} as const)), // expect: no-conditional-empty-object-spread
});

export function filters(status: string | null, from: Date | null): object[] {
  return [
    {
      ...(status ? { status } : {}), // expect: no-conditional-empty-object-spread
      ...(from // expect: no-conditional-empty-object-spread
        ? { createdAt: { gte: from } }
        : {}),
    },
  ];
}
