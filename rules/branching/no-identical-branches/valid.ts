declare function save(id: string): void;
declare function skip(id: string): void;
declare function log(message: string): void;

export function differ(id: string, isDraft: boolean): void {
  if (isDraft) {
    skip(id);
  } else {
    save(id);
  }
}

export function noElse(id: string, isDraft: boolean): void {
  if (isDraft) {
    save(id);
  }
}

export function bothEmpty(isDraft: boolean): void {
  if (isDraft) {
  } else {
  }
}

export function apart(id: string, kind: string): void {
  if (kind === 'a') {
    save(id);
  } else if (kind === 'b') {
    skip(id);
  } else if (kind === 'c') {
    save(id);
  }
}

export function values(isDraft: boolean): number {
  return isDraft ? 1 : 2;
}

export function fallsThrough(kind: string): number {
  let count = 0;
  switch (kind) {
    case 'a':
      count += 1;
    case 'b':
      count += 1;
  }
  return count;
}

export function grouped(kind: string): number {
  switch (kind) {
    case 'a':
    case 'b':
      return 1;
    case 'c':
      return 2;
    case 'd':
      return 1;
    default:
      return 0;
  }
}

declare function format(value: string): string;
declare function format(value: number): string;

export function overloads(value: string | number | Date): string {
  if (typeof value === 'string') {
    return format(value);
  } else if (typeof value === 'number') {
    return format(value);
  }
  return value instanceof Date ? format(value.getTime()) : format(value.getTime());
}

export function byType(value: string | number): string {
  switch (typeof value) {
    case 'string':
      return format(value);
    case 'number':
      return format(value);
    default:
      return '';
  }
}

export function otherMessage(isDraft: boolean): void {
  if (isDraft) {
    log('draft');
  } else {
    log('final');
  }
}

export function byTest(value: string | number): string {
  switch (true) {
    case typeof value === 'string':
      return format(value);
    case typeof value === 'number':
      return format(value);
    default:
      return '';
  }
}

export function byMember(box: { value: string | number }): string {
  return typeof box.value === 'string' ? format(box.value) : format(box.value);
}

export class Field {
  constructor(private readonly value: string | number) {}

  text(): string {
    return typeof this.value === 'string' ? format(this.value) : format(this.value);
  }
}
