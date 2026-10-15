function parseJSON(text: string): unknown { // expect: acronym-case
  return JSON.parse(text);
}

interface APIResponse { // expect: acronym-case
  body: string;
}

class HTTPClient { // expect: acronym-case
  private rawURL = ''; // expect: acronym-case

  send(): APIResponse {
    return { body: this.rawURL };
  }
}

function loadHTTPURL(): string { // expect: acronym-case
  return new HTTPClient().send().body;
}

function readETHUSD(): number { // expect: acronym-case
  return Number(loadHTTPURL());
}

type ABIItem = { name: string }; // expect: acronym-case

export function describe(userID: string, items: ABIItem[]): unknown { // expect: acronym-case
  const orderIDs = items.map((item) => item.name); // expect: acronym-case
  return parseJSON(JSON.stringify({ userID, orderIDs, rate: readETHUSD() }));
}

const baseURL = '/api'; // expect: acronym-case
export { baseURL };
