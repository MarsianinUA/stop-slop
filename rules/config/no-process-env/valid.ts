import type { ConfigService } from '@nestjs/config';
import type { env as Environment } from 'node:process';

declare const config: ConfigService;

// The port comes from config: process.env stays in *.config.ts files.
export function port(): number {
  return config.getOrThrow<number>('app.port');
}

export function cwd(): string {
  return process.cwd();
}

export function onExit(handler: () => void): void {
  process.once('SIGTERM', handler);
}

export function args(): string[] {
  const { argv } = process;
  return argv;
}

export function local(): string | undefined {
  const process = { env: { HOME: '/home' } };
  return process.env.HOME;
}

export type Variables = keyof typeof process.env | keyof typeof Environment;

export function child(spawned: { env: Record<string, string> }): string | undefined {
  return spawned.env.PATH;
}

export function hint(): string {
  return 'process.env.PORT';
}
