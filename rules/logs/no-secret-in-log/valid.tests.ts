// @path src/auth/session.spec.ts
declare const logger: { info(...values: unknown[]): void };

export function logsTheFakeToken(token: string): void {
  logger.info(`pushing with ${token}`);
}
