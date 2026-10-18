export const MAX_ID = 100;
export type ID = string;

interface IUser {
  id: ID;
}

export class Order {
  toJSON(): string {
    return 'order';
  }

  getXAxis(user: IUser): string {
    return user.id;
  }
}

export function loadHttpUrl(userId: ID, i18nKey: string, base64Body: string): string {
  return `${userId}${i18nKey}${base64Body}${MAX_ID}`;
}

const response = { userID: '1', baseURL: '/api' };
export const userId = response.userID;
