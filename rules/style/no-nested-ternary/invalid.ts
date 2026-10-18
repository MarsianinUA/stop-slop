declare const count: number;
declare const isAdmin: boolean;
declare const isOwner: boolean;
declare const flag: boolean;
declare const done: boolean;
declare function format(text: string): string;

export const size = count > 100 ? 'large' : count > 10 ? 'medium' : 'small'; // expect: no-nested-ternary
export const role = isAdmin ? (isOwner ? 'owner' : 'admin') : 'user'; // expect: no-nested-ternary
export const first = (flag ? count : 0) > 1 ? 'many' : 'few'; // expect: no-nested-ternary
export const title = isAdmin ? format(done ? 'done' : 'open') : 'none'; // expect: no-nested-ternary
export const level = count > 3 ? 'high' : count > 2 ? 'medium' : count > 1 ? 'low' : 'none'; // expect: no-nested-ternary
export const note = done // expect: no-nested-ternary
  ? `closed by ${isOwner ? 'owner' : 'admin'}`
  : 'open';
