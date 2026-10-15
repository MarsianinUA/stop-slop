function parseJson(text: string): unknown {
  return JSON.parse(text);
}

interface ApiResponse {
  body: string;
}

class HttpClient {
  private rawURL = '';

  send(): ApiResponse {
    return { body: this.rawURL };
  }
}

function loadHttpUrl(): string {
  return new HttpClient().send().body;
}

function readETHUSD(): number {
  return Number(loadHttpUrl());
}

type AbiItem = { name: string };

export function describe(userId: string, items: AbiItem[]): unknown {
  const orderIds = items.map((item) => item.name);
  return parseJson(JSON.stringify({ userID: userId, orderIDs: orderIds, rate: readETHUSD() }));
}

const baseURL = '/api';
export { baseURL };
