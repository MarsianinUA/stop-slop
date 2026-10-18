// @path src/orders/order-button.component.tsx
declare function audit(row: string): void;

function save(): void {
  audit('save');
}

export function OrderButton() {
  return (
    <button
      onClick={() => {
        save();
      }}
    >
      Save
    </button>
  );
}
