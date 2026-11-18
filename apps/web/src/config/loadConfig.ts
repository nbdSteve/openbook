export interface DemoSource {
  title: string;
  license: string;
  url: string;
}

export interface DemoConfig {
  enabled: boolean;
  sources: DemoSource[];
  examples: string[];
  model?: { label: string };
}

export interface AppConfig {
  stage: string;
  region: string;
  cognito: {
    userPoolId: string;
    clientId: string;
    endpoint: string | null;
  };
  demo?: DemoConfig;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigError';
  }
}

function isString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0;
}

export function parseDemo(value: unknown): DemoConfig | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const v = value as Record<string, unknown>;
  if (v.enabled !== true) return undefined;
  const sources = Array.isArray(v.sources)
    ? v.sources.flatMap((item): DemoSource[] => {
        const s = item as Partial<DemoSource> | null;
        return s && isString(s.title) && isString(s.license) && isString(s.url) ? [{ title: s.title, license: s.license, url: s.url }] : [];
      })
    : [];
  const examples = Array.isArray(v.examples) ? v.examples.filter((e): e is string => isString(e)) : [];
  const model = v.model as { label?: unknown } | null | undefined;
  return { enabled: true, sources, examples, ...(model && isString(model.label) ? { model: { label: model.label } } : {}) };
}

export function parseConfig(value: unknown): AppConfig {
  const v = value as Partial<AppConfig> | null;
  const c = v?.cognito as Partial<AppConfig['cognito']> | undefined;
  if (!v || typeof v !== 'object' || !c || typeof c !== 'object') {
    throw new ConfigError('The configuration file has the wrong shape.');
  }
  if (!isString(v.stage) || !isString(v.region) || !isString(c.userPoolId) || !isString(c.clientId)) {
    throw new ConfigError('The configuration file is missing stage, region, or Cognito settings.');
  }
  if (c.endpoint !== null && c.endpoint !== undefined && !isString(c.endpoint)) {
    throw new ConfigError('The Cognito endpoint must be a string or null.');
  }
  const demo = parseDemo((value as { demo?: unknown }).demo);
  return {
    stage: v.stage,
    region: v.region,
    cognito: { userPoolId: c.userPoolId, clientId: c.clientId, endpoint: c.endpoint ?? null },
    ...(demo ? { demo } : {}),
  };
}

export async function loadConfig(fetchFn: typeof fetch = fetch): Promise<AppConfig> {
  let response: Response;
  try {
    response = await fetchFn('/config.json', { cache: 'no-store' });
  } catch {
    throw new ConfigError('Could not load /config.json. Check your connection and reload.');
  }
  if (!response.ok) {
    throw new ConfigError(`Could not load /config.json (status ${response.status}).`);
  }
  let json: unknown;
  try {
    json = await response.json();
  } catch {
    throw new ConfigError('/config.json is not valid JSON.');
  }
  return parseConfig(json);
}

let current: AppConfig | null = null;

export function setConfig(config: AppConfig): void {
  current = config;
}

export function getConfig(): AppConfig {
  if (!current) throw new ConfigError('The configuration is not loaded yet.');
  return current;
}
