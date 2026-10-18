interface User { id: string }
declare const input: string;

const user = input as unknown as User; // expect: no-chained-type-assertions
const wrapped = (input as unknown) as User; // expect: no-chained-type-assertions
const angled = <User>(<unknown>input); // expect: no-chained-type-assertions
const mixed = ({ id: 'a' } as const) as User; // expect: no-chained-type-assertions
const triple = input as unknown as object as User; // expect: no-chained-type-assertions

function load(): User {
  return input as unknown as User; // expect: no-chained-type-assertions
}

// SAFETY:
const empty = input as unknown as User; // expect: no-chained-type-assertions

export { user, wrapped, angled, mixed, triple, load, empty };
