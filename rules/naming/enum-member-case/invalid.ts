export enum Chain {
  ETHEREUM = 1, // expect: enum-member-case
  ArbitrumOne = 42161,
  base_mainnet = 8453, // expect: enum-member-case
  polygon = 137, // expect: enum-member-case
  OP_MAINNET = 10, // expect: enum-member-case
}

export enum Method {
  GET = 'GET', // expect: enum-member-case
  Post = 'POST',
}

export const enum Level {
  infoLevel, // expect: enum-member-case
  Warn,
}
