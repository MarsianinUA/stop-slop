// @path src/config/app.config.ts
export const appConfig = {
  port: Number(process.env.PORT ?? 3000),
  host: process.env.HOST ?? 'localhost',
};
