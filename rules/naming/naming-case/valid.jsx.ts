// @path src/ui/order-card.component.tsx
import { icons } from './icons';

export function OrderCard({ kind }: { kind: 'star' | 'cart' }) {
  const Icon = icons[kind];
  let Badge = icons.cart;
  if (kind === 'star') {
    Badge = icons.star;
  }
  return (
    <div>
      <Icon />
      <Badge></Badge>
    </div>
  );
}
