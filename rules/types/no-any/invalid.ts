let loose: any; // expect: no-any

function read(input: any): any { // expect: no-any x2
  return input;
}

const items: Array<any> = []; // expect: no-any
const value = read(items) as any; // expect: no-any
