type Env = Record<string, string | undefined>;

export function port(): number {
  return Number(process.env.PORT ?? 3000); // expect: no-process-env
}

export function databaseUrl(name: string): string | undefined {
  return process.env[`DB_${name}_URL`]; // expect: no-process-env
}

export function redis(): { host?: string; port?: string } {
  const { REDIS_HOST, REDIS_PORT } = process.env; // expect: no-process-env
  return { host: REDIS_HOST, port: REDIS_PORT };
}

export function environment(): Env {
  const { env } = process; // expect: no-process-env
  return env;
}

export const isProduction = process.env.NODE_ENV === 'production'; // expect: no-process-env

export function setLogLevel(level: string): void {
  process.env.LOG_LEVEL = level; // expect: no-process-env
}

export function hasToken(): boolean {
  return 'API_TOKEN' in process.env; // expect: no-process-env
}

export function snapshot(): Env {
  return { ...(process.env as Env) }; // expect: no-process-env
}

export function region(): string | undefined {
  return process?.env?.AWS_REGION ?? process['env'].AWS_DEFAULT_REGION; // expect: no-process-env x2
}

export function shell(): string | undefined {
  return (process as NodeJS.Process).env.SHELL; // expect: no-process-env
}
