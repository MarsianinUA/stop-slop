declare const users: Map<string, { name: string }>;
declare const list: string[];

const first = list[0]!; // expect: no-non-null-assertion
const user = users.get('ann')!; // expect: no-non-null-assertion
const name = users.get('bob')!.name; // expect: no-non-null-assertion
const initial = users.get('cid')!.name.at(0)!; // expect: no-non-null-assertion x2

function pick(key: string): string {
  return users.get(key)!.name; // expect: no-non-null-assertion
}

// SAFETY:
const empty = list[1]!; // expect: no-non-null-assertion

// SAFETY: a blank line breaks the link to the code.

const far = list[2]!; // expect: no-non-null-assertion

// safety: lower case is not the marker.
const lower = list[3]!; // expect: no-non-null-assertion

const previous = list.length; // SAFETY: this explains its own line only.
const next = list[4]!; // expect: no-non-null-assertion

// SAFETY: this is above the statement, not above the line with `!`.
const parts = [
  list[5]!, // expect: no-non-null-assertion
];

const prose = list[6]!; // the list is never empty // expect: no-non-null-assertion

export { first, user, name, initial, pick, empty, far, lower, previous, next, parts, prose };
