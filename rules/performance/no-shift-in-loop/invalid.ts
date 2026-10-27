interface TreeNode {
  children: TreeNode[];
}

declare function visit(node: TreeNode): void;

export function walk(root: TreeNode): void {
  const queue = [root];
  while (queue.length > 0) {
    const node = queue.shift(); // expect: no-shift-in-loop
    if (node) {
      visit(node);
      queue.push(...node.children);
    }
  }
}

export function drain(pending: string[]): void {
  while (pending.length) pending.shift(); // expect: no-shift-in-loop
}

export function take(items: number[]): number {
  let total = 0;
  let item: number | undefined;
  while ((item = items.shift()) !== undefined) total += item; // expect: no-shift-in-loop
  return total;
}

export function grow(stack: string[], limit: number): void {
  do {
    stack.unshift('x'); // expect: no-shift-in-loop
  } while (stack.length < limit);
}

export function byIndex(lines: string[]): void {
  for (let i = 0; i < lines.length; i++) {
    lines.shift(); // expect: no-shift-in-loop
  }
}
