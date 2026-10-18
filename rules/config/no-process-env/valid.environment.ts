// @path src/config/environment.ts
export const environment = {
  name: process.env.NODE_ENV ?? 'development',
  region: process.env.AWS_REGION,
};
