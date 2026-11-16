export interface AppConfig {
  stage: string;
  region: string;
  cognito: {
    userPoolId: string;
    clientId: string;
    /** `null` for deployed stages. Locally it points at the Vite proxy for AWS Emulator. */
    endpoint: string | null;
  };
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

/** Check that a parsed JSON value has the shape in docs/api.md. */
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
  return {
    stage: v.stage,
    region: v.region,
    cognito: { userPoolId: c.userPoolId, clientId: c.clientId, endpoint: c.endpoint ?? null },
  };
}

/** Fetch `/config.json`. Throws ConfigError with a message fit for the error screen. */
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
