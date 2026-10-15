interface Options { label?: string; retries?: number }
declare const options: Options | undefined;
declare const env: { NAME?: string; HOST?: string };

const label = options?.label || 'default'; // expect: prefer-nullish-coalescing
const retries = options?.retries || 3; // expect: prefer-nullish-coalescing
const settings = options || {}; // expect: prefer-nullish-coalescing
const name = options?.label || env.NAME || 'anonymous'; // expect: prefer-nullish-coalescing

function host(): string {
  return env.HOST || 'localhost'; // expect: prefer-nullish-coalescing
}

let cached: string | undefined;
cached ||= 'empty'; // expect: prefer-nullish-coalescing

export { label, retries, settings, name, host, cached };
