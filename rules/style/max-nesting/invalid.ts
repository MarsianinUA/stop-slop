interface Item {
  tags: string[];
}
interface Order {
  paid: boolean;
  items: Item[];
}
declare const orders: Order[];
declare function audit(tag: string): void;

export function gifts(): number {
  let count = 0;
  for (const order of orders) {
    if (order.paid) {
      for (const item of order.items) {
        for (const tag of item.tags) {
          if (tag === 'gift') { // expect: max-nesting
            // The levels below belong to the finding above.
            while (count < 10) {
              count += 1;
            }
          } else if (tag === 'sale') {
            count -= 1;
          }
        }
      }
    }
  }
  return count;
}

export function callbacks(): void {
  orders.forEach((order) => {
    order.items.forEach(function (item) {
      item.tags.forEach((tag) => {
        if (tag.length > 0) {
          switch (tag) { // expect: max-nesting
            case 'gift':
              audit(tag);
              break;
            default:
              break;
          }
        }
      });
    });
  });
}

export function guarded(): void {
  try {
    for (const order of orders) {
      if (!order.paid) {
        continue;
      } else {
        for (const item of order.items) {
          item.tags.map(function (tag) { // expect: max-nesting
            return tag.length;
          });
          try { // expect: max-nesting
            audit('paid');
          } catch {
            audit('failed');
          }
        }
      }
    }
  } catch {
    audit('stopped');
  }
}

export function kinds(): void {
  for (const order of orders) {
    switch (order.items.length) {
      case 0:
        for (const item of order.items) {
          if (item.tags.length > 0) {
            while (order.paid) { // expect: max-nesting
              audit('loop');
            }
          }
        }
        break;
      default:
        break;
    }
  }
}

export function retries(): void {
  for (const order of orders) {
    for (const item of order.items) {
      while (order.paid) {
        if (item.tags.length === 0) {
          break;
        } else if (item.tags.length === 1) {
          do { // expect: max-nesting
            audit('one');
          } while (orders.length > 1);
          for (let index = 0; index < 3; index++) { // expect: max-nesting
            audit('retry');
          }
        }
      }
    }
  }
}

// An arrow with an expression body holds no block; the callback with a block inside it does.
export function labels(): string[][] {
  const all: string[][] = [];
  for (const order of orders) {
    for (const item of order.items) {
      if (order.paid) {
        while (all.length < 10) {
          all.push(
            item.tags.map((tag) =>
              [tag].map((name) => { // expect: max-nesting
                return name.trim();
              }),
            ).flat(),
          );
        }
      }
    }
  }
  return all;
}
