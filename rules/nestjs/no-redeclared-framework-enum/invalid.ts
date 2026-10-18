export enum HttpStatus { // expect: no-redeclared-framework-enum
  OK = 200,
  CREATED = 201,
  NOT_FOUND = 404,
}

export function isMissing(status: number): boolean {
  const enum HttpStatus { // expect: no-redeclared-framework-enum
    NOT_FOUND = 404,
  }
  return status === HttpStatus.NOT_FOUND;
}
