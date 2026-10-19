// @path src/ui/order-badges.component.tsx
interface Order {
  isPaid: boolean;
  isShipped: boolean;
  isLate: boolean;
  isGift: boolean;
  note: string;
}

// 3 here: SonarJS leaves out a chain of one operator right in JSX braces.
export function OrderBadges({ order }: { order: Order }) {
  return (
    <ul>
      {order.isPaid && <li>paid</li>}
      {order.isShipped && <li>shipped</li>}
      {order.isLate && <li>late</li>}
      {order.isGift && order.note && <li>{order.note}</li>}
      {order.isPaid && order.isShipped && <li>done</li>}
      {order.isLate && order.isGift && <li>late gift</li>}
      {order.isPaid && order.isLate && <li>paid late</li>}
      {order.isShipped && order.isLate && <li>shipped late</li>}
      {order.isGift && order.isPaid && <li>paid gift</li>}
      {order.isGift && order.isShipped && <li>shipped gift</li>}
      {order.note && <li>{order.note}</li>}
      {order.isPaid && order.isShipped && order.isLate && <li>all</li>}
      {order.isGift || <li>not a gift</li>}
      {order.isLate || order.isPaid || <li>open</li>}
      {order.isPaid && order.isGift && order.note && <li>{order.note}</li>}
      {order.isShipped && order.isGift && order.note && <li>{order.note}</li>}
      {order.isLate && (order.isGift ? <li>late gift</li> : <li>late</li>)}
      {(order.isPaid && order.isGift) || <li>no paid gift</li>}
    </ul>
  );
}
