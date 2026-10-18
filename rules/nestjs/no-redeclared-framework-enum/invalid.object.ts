// @path src/http/http-status.constants.ts
export const HttpStatus = { // expect: no-redeclared-framework-enum
  OK: 200,
  ACCEPTED: 202,
} as const;

export type HttpStatus = (typeof HttpStatus)[keyof typeof HttpStatus]; // expect: no-redeclared-framework-enum
