export const isInactive = false;
export const isNotification = true;
export const hasNotes = false;
export const isNoop = true;
export const cannotRetry = false;

export function isNonce(value: string): boolean {
  return value.length === 32;
}

export function isNoScriptError(message: string): boolean {
  return message.startsWith('NOSCRIPT');
}

export class Panel {
  isNotOpen = false;
  protected hasNoRows = true;

  toggle(): boolean {
    this.isNotOpen = !this.hasNoRows;
    return this.isNotOpen;
  }
}

type Flags = { isNotReady: boolean };
export const flags: Flags = { isNotReady: false };
declare const isNotLoaded: boolean;
export const loaded = !isNotLoaded;

export function read({ isNotDone }: { isNotDone: boolean }): boolean {
  const { isNotEmpty } = { isNotEmpty: isNotDone };
  return isNotEmpty;
}
