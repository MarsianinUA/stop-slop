const RETRY_LIMIT = 3;

export class OrderQueue {
  attempts(): number {
    return RETRY_LIMIT;
  }
}
