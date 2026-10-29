// The stand of stop-slop mounts /stand/out and sets STOP_SLOP_* variables: a comment is no code.
export const standard = '/standard';
export const stands = '/stands/42';
export const word = 'stand';
export const grandstand = 'grandstand/north';
export const sandbox = 'sandbox';
export const preload = './preload.js';
export const lower = 'stop_slop';
export const tool = 'stop-slop';

export function report(id: string): string {
  return `/standard/${id}`;
}

const STOP = 1;
export const slop = STOP + 1;

// A part of a template is never the whole text, so only `/stand/` in it counts.
export function tail(root: string): string {
  return `${root}/stand`;
}

export function head(suffix: string): string {
  return `/stand${suffix}`;
}
