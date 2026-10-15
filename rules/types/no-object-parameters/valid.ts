interface Owner { readonly id: string }
declare function validateIf(check: (...args: never[]) => boolean): void;

function load(owner: Owner): string {
  return owner.id;
}

function first<T extends object>(items: T[]): T | undefined {
  return items[0];
}

function isOwner(value: object): value is Owner {
  return 'id' in value;
}

validateIf((_target: object, value: unknown) => value !== undefined);

function Cursor(): PropertyDecorator {
  return (target: object, propertyName: string | symbol) => {
    void target;
    void propertyName;
  };
}

[{}].forEach((item: object) => void item);

const ignoreTarget = () => (target: object) => void target;

const describe = (input: unknown): string => typeof input;

type Alias = object;
function shadowed<Alias>(value: Alias): Alias {
  return value;
}

export { load, first, isOwner, Cursor, ignoreTarget, describe, shadowed };
