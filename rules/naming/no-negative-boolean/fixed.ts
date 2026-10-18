export const isNotSupported = true;

function label(isInactive: boolean): string {
  return isInactive ? 'idle' : 'busy';
}

const isInvalid = label(false) === 'idle';
const hasNoRows = [label(true)].length === 0;
const isNonEmpty = !hasNoRows;
const canNotRetry = !isInvalid && isNonEmpty;
const IS_HIDDEN = canNotRetry;
const is_not_active = !IS_HIDDEN;

export function isNotBlank(value: string): boolean {
  return value !== '' && is_not_active;
}

export function toggle(isNotEnabled: boolean): boolean {
  const disabled = !isNotEnabled;
  return disabled;
}

export class Panel {
  private isNotOpen = false;

  open(): boolean {
    this.isNotOpen = !this.isNotOpen;
    return this.isNotOpen;
  }
}

export function compare(left: string, right: string): string {
  const isNotEqual = left !== right;
  const isNotSame = left.length !== right.length;
  return isNotEqual && isNotSame ? 'differ' : 'same';
}

export function lookup(status: number): string {
  const isNotFoundError = status === 404;
  return isNotFoundError ? 'gone' : 'found';
}
