export const isNotSupported = true; // expect: no-negative-boolean

function label(isNotActive: boolean): string { // expect: no-negative-boolean
  return isNotActive ? 'idle' : 'busy';
}

const isNotValid = label(false) === 'idle'; // expect: no-negative-boolean
const hasNoRows = [label(true)].length === 0; // expect: no-negative-boolean
const isNonEmpty = !hasNoRows; // expect: no-negative-boolean
const canNotRetry = !isNotValid && isNonEmpty; // expect: no-negative-boolean
const IS_NOT_VISIBLE = canNotRetry; // expect: no-negative-boolean
const is_not_active = !IS_NOT_VISIBLE; // expect: no-negative-boolean

export function isNotBlank(value: string): boolean { // expect: no-negative-boolean
  return value !== '' && is_not_active;
}

export function toggle(isNotEnabled: boolean): boolean { // expect: no-negative-boolean
  const disabled = !isNotEnabled;
  return disabled;
}

export class Panel {
  private isNotOpen = false; // expect: no-negative-boolean

  open(): boolean {
    this.isNotOpen = !this.isNotOpen;
    return this.isNotOpen;
  }
}

export function compare(left: string, right: string): string {
  const isNotEqual = left !== right; // expect: no-negative-boolean
  const isNotSame = left.length !== right.length; // expect: no-negative-boolean
  return isNotEqual && isNotSame ? 'differ' : 'same';
}

export function lookup(status: number): string {
  const isNotFoundError = status === 404; // expect: no-negative-boolean
  return isNotFoundError ? 'gone' : 'found';
}
