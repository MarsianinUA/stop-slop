interface Row {
  skip: boolean;
  size: number;
  tags?: { name: string }[];
}

type Size<T> = T extends string ? 'text' : 'other';

// 15, the limit.
export function atLimit(rows: Row[], mode: string): number {
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
  return total || rows.length || 0;
}

// `||`, `??`, `?.` and conditional types are free.
export function free(row: Row | null, fallback: number): Size<string> | number {
  const size = row?.size ?? row?.tags?.length ?? fallback;
  const name = row?.tags?.[0]?.name || 'none';
  return size || name.length || fallback;
}

// 6 here and 10 in the callback: a nested function counts on its own.
export function apart(rows: Row[], limit: number): number[] {
  if (rows.length === 0) { // +1 if
    return [];
  } else { // +1 else
    limit -= 1;
  }
  for (const row of rows) { // +1 for
    if (row.skip && limit > 0) limit -= 1; // +2 if, nested once; +1 &&
  }
  return rows.map((row) => {
    if (row.size > 0) { // +1
      if (row.size > 10) { // +2
        if (row.size > 100) { // +3
          if (row.size > 1000) return 4; // +4
          return 3;
        }
        return 2;
      }
      return 1;
    }
    return 0;
  });
}
