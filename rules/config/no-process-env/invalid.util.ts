// @path src/shared/runtime.util.ts
export function scrubSecrets(text: string): string {
  const secret = process.env.API_SECRET; // expect: no-process-env
  return secret ? text.replaceAll(secret, '***') : text;
}
