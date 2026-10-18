export function run(): number {
  let total = 0;
  { // expect: no-bare-block
    const step = 2;
    total += step;
  }
  if (total > 1) {
    { // expect: no-bare-block
      total -= 1;
    }
  }
  return total;
}

{ // expect: no-bare-block
  const local = 1;
  void local;
}

export function kind(code: number): string {
  switch (code) {
    case 1:
      void code;
      { // expect: no-bare-block
        const name = 'one';
        return name;
      }
    default:
      return 'other';
  }
}

export class Cache {
  static size = 0;
  static {
    { // expect: no-bare-block
      Cache.size = 10;
    }
  }
}

export namespace Limits {
  { // expect: no-bare-block
    void Limits;
  }
}
