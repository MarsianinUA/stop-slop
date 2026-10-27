interface Row {
  skip: boolean;
  size: number;
}

// 16: one point past the limit.
export function overLimit(rows: Row[], mode: string): number { // expect: max-cognitive-complexity
  let total = 0;
  outer: for (const row of rows) { // +1 for
    if (row.skip) { // +2 if, nested once
      continue outer; // +1 continue to a label
    } else if (row.size > 10) { // +1 else if
      total += row.size > 100 ? 2 : 1; // +3 ?:, nested twice
    } else { // +1 else
      total += 1;
    }
  }
  switch (mode) { // +1 switch
    case 'strict':
      if (total > 0 && rows.length > 0) { // +2 if, nested once; +1 &&
        total -= 1;
      }
      break;
  }
  try {
    total += rows.length;
  } catch { // +1 catch
    total = 0;
  }
  while (total > 1000) total /= 2; // +1 while
  if (total < 0) total = 0; // +1 if
  return total;
}

// 1 here; the callback has 16 of its own.
export function sizes(rows: Row[]): number[] {
  if (rows.length === 0) return []; // +1 if
  return rows.map((row) => { // expect: max-cognitive-complexity
    let size = 0;
    for (let i = 0; i < row.size; i++) { // +1 for
      if (i > 1) { // +2
        if (i > 10) { // +3
          if (i > 100) { // +4
            size += i > 1000 ? 2 : 1; // +5 ?:, nested four times
          }
        }
      }
    }
    if (row.skip) size = 0; // +1 if
    return size;
  });
}

export class Report {
  build(rows: Row[]): number { // expect: max-cognitive-complexity
    let size = 0;
    for (const row of rows) { // +1 for
      if (row.size > 1) { // +2
        if (row.size > 10) { // +3
          if (row.size > 100) { // +4
            size += row.skip ? 2 : 1; // +5 ?:, nested four times
          }
        }
      }
    }
    if (rows.length === 0) size = 0; // +1 if
    return size;
  }
}

export const total = (rows: Row[]): number => { // expect: max-cognitive-complexity
  let size = 0;
  rows.forEach(() => {
    size += 1;
  });
  for (const row of rows) { // +1 for
    if (row.size > 1) { // +2
      if (row.size > 10) { // +3
        if (row.size > 100) { // +4
          size += row.skip ? 2 : 1; // +5 ?:, nested four times
        }
      }
    }
  }
  if (rows.length === 0) size = 0; // +1 if
  return size;
};
