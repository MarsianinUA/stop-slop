export function requestOptions(timeout: number | undefined): object {
  return { method: 'GET', timeout: timeout ?? null };
}

export function merged(base: object, extra: object | undefined): object {
  return { ...base, ...(extra ?? {}) };
}

export function chosen(isDraft: boolean): object {
  return isDraft ? { status: 'draft' } : {};
}

export function variant(isDraft: boolean): object {
  return { ...(isDraft ? { status: 'draft' } : { status: 'final' }) };
}

export function items(isFull: boolean, all: number[]): number[] {
  return [...(isFull ? all : [])];
}

export function largest(isFull: boolean, all: number[]): number {
  return Math.max(...(isFull ? all : []));
}

export function built(timeout: number | undefined): object {
  const options: { method: string; timeout?: number } = { method: 'GET' };
  if (timeout !== undefined) {
    options.timeout = timeout;
  }
  return options;
}
