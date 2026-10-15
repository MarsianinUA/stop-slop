// @path src/guards/__tests__/api-key.guard.ts
function contextWith(request: object): { request: object } {
  return { request };
}

export { contextWith };
