interface Order {
  id: string;
}

interface IPAddress {
  value: string;
}

interface IPv4Address {
  host: string;
}

interface IOStream {
  write(chunk: string): void;
}

interface I18nConfig {
  locale: string;
}

type IStatus = { open: boolean };

declare global {
  interface IWindowState {
    ready: boolean;
  }
}

export function describe(order: Order, ip: IPAddress, v4: IPv4Address, out: IOStream, i18n: I18nConfig, status: IStatus): void {
  out.write(`${order.id} ${ip.value} ${v4.host} ${i18n.locale} ${status.open}`);
}
