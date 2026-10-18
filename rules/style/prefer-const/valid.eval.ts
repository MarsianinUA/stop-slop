export function run(code: string): number {
  // A direct `eval` may assign `result` while the code runs.
  let result = 0;
  eval(code);
  return result;
}
