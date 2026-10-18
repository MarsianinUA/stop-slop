interface Item {
  tags: string[];
}
interface Order {
  paid: boolean;
  items: Item[];
}
declare const orders: Order[];
declare function audit(tag: string): void;

export function four(): number {
  let count = 0;
  for (const order of orders) {
    if (order.paid) {
      for (const item of order.items) {
        for (const tag of item.tags) {
          count += tag.length;
        }
      }
    }
  }
  return count;
}

// `else if` stays on the level of its `if`.
export function kinds(): void {
  for (const order of orders) {
    for (const item of order.items) {
      if (item.tags.length === 0) {
        audit('none');
      } else if (item.tags.length === 1) {
        if (order.paid) {
          audit('one');
        }
      } else if (item.tags.length === 2) {
        audit('two');
      } else {
        audit('many');
      }
    }
  }
}

// A function that is not a callback starts from zero.
export function nested(): void {
  for (const order of orders) {
    if (order.paid) {
      for (const item of order.items) {
        const check = (tag: string): void => {
          if (tag.length > 0) {
            for (const char of tag) {
              audit(char);
            }
          }
        };
        function report(tag: string): void {
          if (tag) {
            audit(tag);
          }
        }
        item.tags.forEach(check);
        item.tags.forEach(report);
      }
    }
  }
}

// An arrow with an expression body holds no block.
export function short(): number {
  let count = 0;
  for (const order of orders) {
    if (order.paid) {
      for (const item of order.items) {
        for (const tag of item.tags) {
          count += order.items.filter((other) => other.tags.includes(tag)).length;
        }
      }
    }
  }
  return count;
}

// A callback in a condition is not inside the branch.
export function tests(): void {
  for (const order of orders) {
    for (const item of order.items) {
      if (order.paid) {
        if (item.tags.some((tag) => tag === 'gift')) {
          audit('gift');
        }
      }
    }
  }
}

// A field initializer starts from zero.
export function models(): unknown[] {
  const made: unknown[] = [];
  for (const order of orders) {
    if (order.paid) {
      for (const item of order.items) {
        for (const tag of item.tags) {
          made.push(
            class {
              rows = orders.map((other) => {
                if (other.paid) {
                  return tag;
                }
                return '';
              });
            },
          );
        }
      }
    }
  }
  return made;
}
