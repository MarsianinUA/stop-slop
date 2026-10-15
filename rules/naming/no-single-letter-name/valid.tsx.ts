// @path src/ui/price-input.component.tsx
declare function save(text: string): void;

export function PriceInput({ value }: { value: string }) {
  return (
    <input
      value={value}
      onChange={(e) => save(e.target.value)}
      onBlur={function (e) { save(e.target.value); }}
    />
  );
}
