export function color(status: string): string {
  if (status === 'open') {
    return 'blue';
  } else if (status === 'paid') {
    return 'green';
  }
  return 'black';
}

export function describe(value: string | null | undefined): string {
  if (value === null) {
    return 'null';
  }
  if (value === undefined) {
    return 'missing';
  }
  if (value === '') {
    return 'empty';
  }
  return value;
}

export function pair(left: string, right: string): number {
  if (left === 'a') {
    return 1;
  }
  if (right === 'b') {
    return 2;
  }
  if (left === 'c') {
    return 3;
  }
  return 0;
}

export function split(kind: string): number {
  if (kind === 'a') {
    return 1;
  }
  const length = kind.length;
  if (kind === 'b') {
    return 2;
  }
  if (kind === 'c') {
    return 3;
  }
  return length;
}

export function range(count: number): string {
  if (count > 10) {
    return 'many';
  } else if (count > 1) {
    return 'few';
  } else if (count === 1) {
    return 'one';
  }
  return 'none';
}
