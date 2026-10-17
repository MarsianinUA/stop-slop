declare const logger: { info(message: string): void };

export function start(port: number): void {
  logger.info(`listening on ${port}`);
}

export function shadowed(console: { info(message: string): void }): void {
  console.info('a parameter named console');
}

export function local(): void {
  const console = { log: (message: string) => logger.info(message) };
  console.log('a local console');
}

export type Sink = typeof console;

export const NAME = 'console.log';
