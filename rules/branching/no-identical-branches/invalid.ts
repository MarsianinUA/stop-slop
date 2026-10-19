declare function save(id: string): void;
declare function skip(id: string): void;
declare function log(message: string): void;

export function same(id: string, isDraft: boolean): void {
  if (isDraft) { // expect: no-identical-branches
    save(id);
  } else {
    save(id);
  }
}

export function commented(id: string, isDraft: boolean): void {
  if (isDraft) { // expect: no-identical-branches
    // A draft is saved too.
    save(id);
  } else {
    /* The final version. */
    save( id );
  }
}

export function quoted(isDraft: boolean): void {
  if (isDraft) log('saved'); // expect: no-identical-branches
  else {
    log("saved");
  }
}

export function chained(id: string, kind: string): void {
  if (kind === 'a') { // expect: no-identical-branches
    save(id);
  } else if (kind === 'b') {
    save(id);
  } else {
    skip(id);
  }
}

export function lastElse(id: string, kind: string): void {
  if (kind === 'a') {
    skip(id);
  } else if (kind === 'b') { // expect: no-identical-branches
    save(id);
  } else {
    save(id);
  }
}

export function ternary(isDraft: boolean): string {
  return isDraft ? 'saved' : 'saved'; // expect: no-identical-branches
}

export function typed(value: string | number): string {
  return typeof value === 'string' ? 'known' : 'known'; // expect: no-identical-branches
}

export function cases(kind: string): number {
  switch (kind) {
    case 'a': // expect: no-identical-branches
      return 1;
    case 'b':
      return 1;
    case 'c':
      return 1;
    case 'd':
      return 2;
    default:
      return 0;
  }
}

export function withDefault(kind: string, id: string): void {
  switch (kind) {
    case 'a':
      save(id);
      break;
    case 'b': // expect: no-identical-branches
      skip(id);
      break;
    default:
      skip(id);
      break;
  }
}
