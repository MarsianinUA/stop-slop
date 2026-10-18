type Log = (...values: unknown[]) => void;
type Request = { headers: Record<string, string | undefined>; cookies: Record<string, string> };
declare const logger: { info: Log; warn: Log; error: Log; debug: Log };
declare const config: { apiKey: string; database: { password: string } };

export function signIn(user: string, password: string): void {
  logger.info(`sign in ${user} with ${password}`); // expect: no-secret-in-log
}

export function refreshed(accessToken: string): void {
  logger.debug('refreshed', accessToken); // expect: no-secret-in-log
}

export function forward(req: Request): void {
  logger.info({ path: '/v1', auth: req.headers.authorization }); // expect: no-secret-in-log
  logger.debug(req.headers['x-api-key']); // expect: no-secret-in-log
  logger.debug({ cookies: req.cookies }); // expect: no-secret-in-log
}

export function connect(): void {
  logger.info({ apiKey: config.apiKey, ...config.database }); // expect: no-secret-in-log
  console.log('db', config.database.password); // expect: no-secret-in-log
}

export function issue(user: string, token: string): void {
  logger.info({ user, token }); // expect: no-secret-in-log
}

export function issueText(user: string, signature: string): void {
  logger.warn('issued for ' + user + ': ' + signature.toString()); // expect: no-secret-in-log
}

export function expire(expired: boolean, refreshToken: string): void {
  logger.info(expired ? 'expired' : refreshToken); // expect: no-secret-in-log
}

export function rotate(previous: { secret: string }, next?: { secret: string }): void {
  logger.info(`rotate ${next?.secret ?? previous.secret}`); // expect: no-secret-in-log x2
  logger.error(JSON.stringify({ privateKey: String(next?.secret) })); // expect: no-secret-in-log
}

export function verify(header?: { signature: string }): void {
  logger.debug(header?.signature.toString()); // expect: no-secret-in-log
}

export function login(id: string, credentials: { user: string; password: string }): void {
  logger.info({ id, ...credentials }); // expect: no-secret-in-log
}

export class Session {
  readonly #token: string;
  private readonly logger = logger;

  constructor(token: string) {
    this.#token = token;
  }

  open(): void {
    this.logger?.info(
      'opened',
      this.#token, // expect: no-secret-in-log
    );
  }
}

export function keys(apiKey: string, secretKey: string): void {
  logger.info('keys', [apiKey, secretKey]); // expect: no-secret-in-log x2
}

export function spread(credentials: string[]): void {
  logger.debug(...credentials); // expect: no-secret-in-log
}

export function listed(id: string, privateKey: string[]): void {
  logger.debug({ id, keys: [id, ...privateKey] }); // expect: no-secret-in-log
}

export function share(ready: boolean, token: string): void {
  logger.info(ready && token); // expect: no-secret-in-log
}
