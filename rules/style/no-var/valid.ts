declare global {
  var requestCount: number;
}
declare var legacyTotal: number;
declare namespace Legacy {
  var version: string;
}

export const total = legacyTotal + globalThis.requestCount;
export let current = Legacy.version;
current = 'var';
