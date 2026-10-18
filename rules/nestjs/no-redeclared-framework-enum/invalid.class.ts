// @path src/http/http-status.ts
function Codes(): ClassDecorator {
  return () => undefined;
}

@Codes()
export class HttpStatus { // expect: no-redeclared-framework-enum
  static readonly OK = 200;
  static readonly NO_CONTENT = 204;
}
