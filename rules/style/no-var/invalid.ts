var total = 0; // expect: no-var

export function sum(values: number[]): number {
  var result = 0; // expect: no-var
  for (var index = 0; index < values.length; index++) { // expect: no-var
    result += values[index];
  }
  return result + total;
}

var { width, height } = { width: 1, height: 2 }; // expect: no-var
var first = 1, second = 2; // expect: no-var
var { // expect: no-var
  depth,
} = { depth: 3 };
for (var key in { a: 1 }) { // expect: no-var
  void key;
}

export { width, height, first, second, depth };
