import axios from 'axios'; // expect: banned-packages
import type { AxiosInstance } from 'axios'; // expect: banned-packages
import { v4 } from 'uuid'; // expect: banned-packages
import get from 'lodash/get'; // expect: banned-packages
import 'dotenv/config'; // expect: banned-packages
export { default as fetchUrl } from 'node-fetch'; // expect: banned-packages

const got = require('got'); // expect: banned-packages

export async function load(client: AxiosInstance): Promise<unknown> {
  const { debounce } = await import('lodash'); // expect: banned-packages
  return [axios, client, v4(), get, got, debounce];
}

export type Client = import('axios').AxiosStatic; // expect: banned-packages
