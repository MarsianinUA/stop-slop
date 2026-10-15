type UserId = string & { readonly brand: 'UserId' };
interface User { id: UserId; name: string }
declare const input: unknown;
declare const raw: string;

const id = raw as UserId; // expect: no-unexplained-as
const user = <User>input; // expect: no-unexplained-as
const forced = input as unknown as User; // expect: no-unexplained-as
const reshaped = ({ name: 'Ann' } as const) as unknown; // expect: no-unexplained-as
const fields = { // expect: no-unexplained-as
  id,
  name: 'Ann',
} as User;

function parse(): User {
  return input as User; // expect: no-unexplained-as
}

// SAFETY:
const empty = raw as UserId; // expect: no-unexplained-as

const early = { // SAFETY: the end of the first line does not count. // expect: no-unexplained-as
  id,
  name: 'Bob',
} as User;

export { id, user, forced, reshaped, fields, parse, empty, early };
