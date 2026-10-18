// @path testing/jest/setup.ts
process.env.LOG_LEVEL ??= 'silent';

jest.mock('@nestjs/config', () => ({ ConfigModule: { forRoot: () => ({}) } }));

export {};
