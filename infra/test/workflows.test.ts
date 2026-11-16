import * as fs from 'fs';
import * as path from 'path';
import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

const root = path.resolve(__dirname, '..', '..');
const read = (file: string): string => fs.readFileSync(path.join(root, file), 'utf8');

interface Step {
  name?: string;
  uses?: string;
  run?: string;
  env?: Record<string, string>;
  with?: Record<string, string>;
  'working-directory'?: string;
}
interface Job {
  environment?: string;
  permissions?: Record<string, string>;
  steps: Step[];
}
interface Workflow {
  on: Record<string, unknown>;
  permissions?: Record<string, string>;
  env?: Record<string, string>;
  concurrency?: { group: string; 'cancel-in-progress': boolean };
  jobs: Record<string, Job>;
}
const load = (name: string): { text: string; wf: Workflow } => {
  const text = read(`.github/workflows/${name}`);
  return { text, wf: parse(text) as Workflow };
};

const checks = load('checks.yml');
const deploy = load('deploy.yml');
const allSteps = (wf: Workflow): Step[] => Object.values(wf.jobs).flatMap((j) => j.steps);

describe.each([
  ['checks.yml', checks],
  ['deploy.yml', deploy],
])('%s', (_name, { wf, text }) => {
  test('pins every third-party action to a full commit SHA', () => {
    const uses = allSteps(wf)
      .map((s) => s.uses)
      .filter((u): u is string => Boolean(u));
    expect(uses.length).toBeGreaterThan(0);
    for (const u of uses) expect(u, u).toMatch(/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/);
  });

  test('grants the workflow read-only contents by default', () => {
    expect(wf.permissions).toEqual({ contents: 'read' });
  });

  test('holds no long-lived AWS credential', () => {
    expect(text).not.toMatch(/AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|secrets\.AWS/);
  });

  test('embeds no account number but the fake one of the placeholder synth', () => {
    expect(text.replaceAll('111111111111', '')).not.toMatch(/\b\d{12}\b/);
  });
});

describe('the checks workflow', () => {
  test('runs on pull requests and pushes to main', () => {
    expect(Object.keys(checks.wf.on).sort()).toEqual(['pull_request', 'push']);
  });

  test('requests no OIDC token and reads no AWS variable, so it reaches no account', () => {
    expect(checks.text).not.toContain('id-token');
    expect(checks.text).not.toMatch(/vars\.AWS|configure-aws-credentials|\bsecrets\./);
    for (const job of Object.values(checks.wf.jobs)) expect(job.permissions).toBeUndefined();
  });

  test('has the five jobs of the documented gate', () => {
    expect(Object.keys(checks.wf.jobs).sort()).toEqual(['e2e-local', 'infra', 'rust', 'rust-integration', 'web']);
  });

  test('runs the Rust commands of AGENTS.md', () => {
    const runs = checks.wf.jobs.rust.steps.map((s) => s.run);
    expect(runs).toContain('cargo fmt --all --check');
    expect(runs).toContain('cargo clippy --workspace --all-targets -- -D warnings');
    expect(runs).toContain('cargo test --workspace');
  });

  test('builds the emulator image fresh and runs the integration tests through make', () => {
    const steps = checks.wf.jobs['rust-integration'].steps.map((s) => s.run ?? '');
    expect(steps.some((r) => r.includes('docker build --pull --no-cache'))).toBe(true);
    expect(steps).toContain('make test-integration');
  });

  test('runs the web and infra checks', () => {
    const web = checks.wf.jobs.web.steps.map((s) => s.run);
    for (const cmd of ['npm ci', 'npm run lint', 'npm run typecheck', 'npm test', 'npm run build']) {
      expect(web).toContain(cmd);
    }
    const infra = checks.wf.jobs.infra.steps.map((s) => s.run ?? '').join('\n');
    for (const cmd of ['npm ci', 'npm run build', 'npm test', 'synth:placeholders']) {
      expect(infra).toContain(cmd);
    }
  });

  test('runs the local end-to-end suite with Playwright Chromium installed first', () => {
    const runs = checks.wf.jobs['e2e-local'].steps.map((s) => s.run ?? '');
    const install = runs.findIndex((r) => r.includes('playwright install') && r.includes('chromium'));
    const e2e = runs.indexOf('make e2e-local');
    expect(install).toBeGreaterThanOrEqual(0);
    expect(e2e).toBeGreaterThan(install);
  });
});

describe('the deploy workflow', () => {
  const job = deploy.wf.jobs.deploy;
  const runs = job.steps.map((s) => s.run ?? '');

  test('runs on pushes to main and manual dispatch only', () => {
    expect(Object.keys(deploy.wf.on).sort()).toEqual(['push', 'workflow_dispatch']);
    expect(deploy.wf.on.push).toEqual({ branches: ['main'] });
  });

  test('never runs two deployments at once and never cancels one', () => {
    expect(deploy.wf.concurrency).toEqual({ group: 'deploy-production', 'cancel-in-progress': false });
  });

  test('deploys through the production environment and asks for an OIDC token on that job alone', () => {
    expect(Object.keys(deploy.wf.jobs)).toEqual(['deploy']);
    expect(job.environment).toBe('production');
    expect(job.permissions).toEqual({ contents: 'read', 'id-token': 'write' });
    expect(deploy.wf.permissions).not.toHaveProperty('id-token');
  });

  test('refuses to run from any ref but main', () => {
    expect(deploy.text).toContain('refs/heads/main');
    expect(job.steps[0].name).toBe('require main');
  });

  test('assumes the role in vars.AWS_DEPLOY_ROLE_ARN through OIDC in ap-southeast-2', () => {
    const step = job.steps.find((s) => s.uses?.startsWith('aws-actions/configure-aws-credentials@'));
    expect(step?.with).toMatchObject({
      'role-to-assume': '${{ vars.AWS_DEPLOY_ROLE_ARN }}',
      'aws-region': 'ap-southeast-2',
      audience: 'sts.amazonaws.com',
    });
  });

  test('checks its variables before it builds or deploys anything', () => {
    const names = job.steps.map((s) => s.name);
    const check = names.indexOf('require the deployment configuration');
    expect(check).toBeGreaterThanOrEqual(0);
    expect(check).toBeLessThan(names.indexOf('build the API Lambda'));
    expect(check).toBeLessThan(names.indexOf('deploy'));
  });

  test('builds the Lambda with a pinned cargo-lambda, then the web app, before it assumes the role', () => {
    expect(deploy.wf.env?.CARGO_LAMBDA_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
    const names = job.steps.map((s) => s.name);
    const lambda = runs.findIndex((r) => r.includes('cargo lambda build --release --arm64 -p openbook-api'));
    const web = names.indexOf('build the web app');
    const assume = names.indexOf('assume the deployment role');
    expect(lambda).toBeGreaterThanOrEqual(0);
    expect(lambda).toBeLessThan(assume);
    expect(web).toBeLessThan(assume);
  });

  test('deploys only Openbook-prod, as the execution role, without prompts', () => {
    const step = job.steps.find((s) => s.name === 'deploy');
    expect(step?.run).toContain('npx cdk deploy Openbook-prod');
    expect(step?.run).toContain('--require-approval never');
    expect(step?.run).toContain('--role-arn "$AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN"');
    expect(step?.env?.AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN).toBe('${{ vars.AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN }}');
    expect(step?.run).not.toMatch(/--all|Openbook-dev/);
  });

  test('smoke tests /api/health for ok: true and / for 200 after the deploy', () => {
    const names = job.steps.map((s) => s.name);
    const deployAt = names.indexOf('deploy');
    const health = job.steps.find((s) => s.name === 'smoke test /api/health');
    const root = job.steps.find((s) => s.name === 'smoke test /');
    expect(health?.run).toContain('/api/health');
    expect(health?.run).toContain('.ok == true');
    expect(root?.run).toContain('!= 200');
    expect(names.indexOf('smoke test /api/health')).toBeGreaterThan(deployAt);
    expect(names.indexOf('smoke test /')).toBeGreaterThan(deployAt);
  });

  test('embeds no role name or ARN', () => {
    expect(deploy.text).not.toMatch(/arn:aws:iam::\d/);
  });
});
