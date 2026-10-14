// any in a comment is not a type
const label = "any value";
const anyway: number = 1;

type Payload = { anyField: string };

function parse(input: unknown): Payload {
  if (typeof input === "object" && input !== null && "anyField" in input) {
    return { anyField: String(input.anyField) };
  }
  return { anyField: `${label} ${anyway}` };
}

const items: Array<string> = [];
const value = parse(items) as Payload;
