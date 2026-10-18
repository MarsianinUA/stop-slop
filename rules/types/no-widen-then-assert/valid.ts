interface Order { readonly id: string }
declare const input: unknown;
declare function make(): unknown;

const parsed = input as Order;

const source = { id: 'first' };
const widened: unknown = source;
const kept = widened;

let changing: unknown = { id: 'second' };
changing = 'other';
const fromLet = changing as Order;

const made: unknown = make();
const fromCall = made as Order;

const outer: unknown = { id: 'third' };
function later(): Order {
  return outer as Order;
}

const stillBroad = widened as unknown;

const explained: unknown = { id: 'fourth' };
// SAFETY: the test harness swaps this value, so its first type is lost on purpose.
const fromExplained = explained as Order;

export { parsed, kept, fromLet, fromCall, later, stillBroad, fromExplained };
