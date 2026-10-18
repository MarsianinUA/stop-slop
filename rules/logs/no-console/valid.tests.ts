// @path src/orders/order.spec.ts
export function silenceWarnings(): () => void {
  const warn = console.warn;
  console.warn = () => undefined;
  return () => {
    console.warn = warn;
  };
}
