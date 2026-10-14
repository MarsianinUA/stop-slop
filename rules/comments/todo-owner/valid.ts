// TODO(denis): move the limit to config
const limit = 10;

/* TODO(team-a) check the rounding */
const rate = 0.2;

/**
 * Adds the tax to the price.
 * TODO(denis): support other currencies
 */
function withTax(price: number): number {
  return price * (1 + rate);
}

const note = "TODO later";
const banner = `TODO ${limit}`;

// todo in lower case is plain text
// TODOS, MYTODO and TODO_LIST are other words
export { banner, limit, note, withTax };
