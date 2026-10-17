// @path src/config/app.config.spec.ts
export function withPort(port: string, run: () => void): void {
  const saved = process.env.PORT;
  process.env.PORT = port;
  run();
  process.env.PORT = saved;
}
