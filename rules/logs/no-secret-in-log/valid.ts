type Log = (...values: unknown[]) => void;
declare const logger: { info: Log; warn: Log; error: Log; flush(token: string): void };
declare const dialog: { error: Log };
declare const audit: { record(token: string): void };
declare function log(...values: unknown[]): void;
declare function mask(value: string): string;

export function signIn(user: string, password: string): void {
  logger.info(`sign in ${user}`, { hasPassword: password.length > 0 });
  logger.info({ password: mask(password) });
  logger.info({ password: password ? '[set]' : '[empty]' });
  logger.info({ user, password: undefined });
}

export function refreshed(accessToken: string, tokenAddress: string, tokenId: number): void {
  logger.info('refreshed', accessToken.length, accessToken.slice(-4));
  logger.info({ tokenAddress, tokenId, symbol: tokenAddress.slice(0, 6) });
  logger.info({ [accessToken]: 1 });
}

export function notLogs(token: string): void {
  dialog.error(token);
  log(token);
  audit.record(token);
  logger.flush(token);
}

export function text(): void {
  logger.warn('the token expired; ask for a new password');
}
