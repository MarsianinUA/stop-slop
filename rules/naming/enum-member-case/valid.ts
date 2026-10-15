export enum Chain {
  Ethereum = 1,
  ArbitrumOne = 42161,
  Base = 8453,
}

export enum Grade {
  A,
  B,
}

export enum Header {
  'content-type' = 'content-type',
  Accept = 'accept',
}

export enum Protocol {
  Http = 'HTTP',
  Https = 'HTTPS',
  Erc20 = 'ERC20',
}

declare enum Legacy {
  OLD_VALUE,
}

declare global {
  enum GlobalLevel {
    DEBUG_LEVEL,
  }
}

export const legacy = Legacy.OLD_VALUE;
