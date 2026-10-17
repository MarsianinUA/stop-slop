// @path src/config/envs/require-env.ts
export function requireEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`Environment variable ${name} is missing`);
  }
  return value;
}
