import { Injectable, Module } from '@nestjs/common';
import type { INestApplication, Provider } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import type { ConfigService as Settings } from '@nestjs/config';

type Env = { app: { baseUrl: string | null; realtimePath: string } };

@Injectable()
export class DiscoveryService {
  private readonly baseUrl: string | undefined;
  private readonly settings: ConfigService<Env, true>;
  readonly #config: ConfigService;

  constructor(private readonly config: ConfigService, settings: Settings<Env, true>) {
    this.baseUrl = this.config.get<string | undefined>('app.baseUrl'); // expect: config-get-or-throw
    this.settings = settings;
    this.#config = config;
    void settings.get('app', { infer: true }); // expect: config-get-or-throw
  }

  realtimePath(): string {
    return this.settings.get('app.realtimePath', { infer: true }); // expect: config-get-or-throw
  }

  docsPath(): string | undefined {
    return this.#config?.get('app.docsPath'); // expect: config-get-or-throw
  }

  timeout(): number {
    return this.config['get']<number>('http.timeoutMs', 5000); // expect: config-get-or-throw
  }

  names(): string[] {
    return ['a', 'b'].map((key) => this.config.get<string>(`names.${key}`) ?? key); // expect: config-get-or-throw
  }
}

export const GithubClientProvider: Provider = {
  provide: 'GITHUB_CLIENT',
  inject: [ConfigService],
  useFactory: (configService: ConfigService) => ({
    token: configService.get<string>('worker.githubToken'), // expect: config-get-or-throw
  }),
};

export async function readPort(app: INestApplication): Promise<number> {
  const configService = app.get(ConfigService);
  const resolved = await app.resolve<ConfigService>(ConfigService);
  const host: string | undefined = app.get(ConfigService).get('worker.host'); // expect: config-get-or-throw
  void host;
  void resolved.get('worker.name'); // expect: config-get-or-throw
  return configService.get<number>('worker.port') ?? 3001; // expect: config-get-or-throw
}

@Module({ providers: [DiscoveryService, GithubClientProvider] })
export class DiscoveryModule {}
