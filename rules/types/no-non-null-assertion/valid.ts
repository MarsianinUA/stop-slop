declare const users: Map<string, { name: string }>;
declare const list: string[];

// SAFETY: the list is filled before this module loads.
const first = list[0]!;

const second = list[1]!; // SAFETY: the length is checked at startup.

/* SAFETY: a block comment works too. */
const third = list[2]!;

// SAFETY: the reason may go on
// over several comment lines.
const fourth = list[3]!;

// SAFETY: other comments may stand between.
// eslint-disable-next-line no-console
const fifth = list[4]!;

const sixth = /* SAFETY: inline, on the same line. */ list[5]!;

const parts = [
  list[6]!, // SAFETY: the end of the line with `!` counts.
];

/**
 * SAFETY: a doc-style block works too.
 */
const seventh = list[7]!;

const name = users.get('ann')?.name ?? 'none';
const negated = !name;
const different = name !== 'none';

let later!: string;
later = 'set';

class Box {
  item!: string;
}

export { first, second, third, fourth, fifth, sixth, parts, seventh, negated, different, later, Box };
