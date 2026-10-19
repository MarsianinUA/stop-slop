export const REPEATED = /^(a+)+$/; // expect: no-nested-quantifier
export const WORDS = /(\w*)*;/; // expect: no-nested-quantifier
export const SLUG = /^([a-z0-9]+[-_]?)+$/; // expect: no-nested-quantifier
export const NUMBERS = /^(\d+(?:\.\d+)?)*$/; // expect: no-nested-quantifier
export const ALTERNATIVE = /^(?:x|y{1,3})+$/; // expect: no-nested-quantifier
export const INNER = /^((?:ab)+|(c+)+)$/; // expect: no-nested-quantifier
export const TWICE = /^(a+)+-(b*)*$/; // expect: no-nested-quantifier x2
export const EMAIL = /^([a-zA-Z0-9]{2,4})+@example\.com$/; // expect: no-nested-quantifier
export const OUTER = /(?:x(a+)+)*$/; // expect: no-nested-quantifier
export const AHEAD = /(a+)+(?=b)/; // expect: no-nested-quantifier
export const BUILT = new RegExp('(x+)+y'); // expect: no-nested-quantifier
export const CALLED = RegExp('^(?:\\s*)+$', 'u'); // expect: no-nested-quantifier
export const TEMPLATE = new RegExp(`^(\\d{1,3})+$`); // expect: no-nested-quantifier
