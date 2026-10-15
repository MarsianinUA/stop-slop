interface ApiKey {
  name: string;
  expiresAt?: string | null; // expect: no-optional-and-null
  revokedAt?: null | Date; // expect: no-optional-and-null
}

type Page = {
  cursor?: string | number | null; // expect: no-optional-and-null
  meta: { total?: number | null }; // expect: no-optional-and-null
};

class Filter {
  status?: string | null; // expect: no-optional-and-null
  private owner?: null; // expect: no-optional-and-null
}

abstract class Base {
  abstract label?: string | null; // expect: no-optional-and-null
}

export type { ApiKey, Page };
export { Filter, Base };
