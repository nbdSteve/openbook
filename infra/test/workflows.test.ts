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
  with?: Record<string, string | boolean>;
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
  jobs: Record<string, Job | (Job & { uses?: string })>;
}
const load = (name: string): { text: string; wf: Workflow } => {
  const text = read(`.github/workflows/${name}`);
  return { text, wf: parse(text) as Workflow };
};

const checks = load('checks.yml');
const deploy = load('deploy.yml');
const allSteps = (wf: Workflow): Step[] => Object.values(wf.jobs).flatMap((j) => j.steps ?? []);

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
    expect(text).not.toMatch(/AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN/);
  });

  test('embeds no account number but the fake one of the placeholder synth', () => {
    expect(text.replaceAll('111111111111', '')).not.toMatch(/\b\d{12}\b/);
  });
});

describe('the checks workflow', () => {
  test('runs on pull requests and when another workflow calls it, and not on push', () => {
    expect(Object.keys(checks.wf.on).sort()).toEqual(['pull_request', 'workflow_call']);
  });

  test('only a pull request may cancel it, so a deploy never cancels its own gate', () => {
    expect(checks.wf.concurrency?.['cancel-in-progress']).toBe("${{ github.event_name == 'pull_request' }}");
    expect(checks.wf.concurrency?.group).toContain('github.workflow');
  });

  test('requests no OIDC token and reads no AWS secret or variable, so it reaches no account', () => {
    expect(checks.text).not.toContain('id-token');
    expect(checks.text).not.toMatch(/vars\.|configure-aws-credentials|\bsecrets\./);
    for (const job of Object.values(checks.wf.jobs)) expect(job.permissions).toBeUndefined();
  });

  test('has the seven jobs of the documented gate', () => {
    expect(Object.keys(checks.wf.jobs).sort()).toEqual([
      'e2e-local',
      'infra',
      'lambda',
      'rust',
      'rust-integration',
      'scripts',
      'web',
    ]);
  });

  test('runs the Rust commands of AGENTS.md', () => {
    const runs = checks.wf.jobs.rust.steps.map((s) => s.run);
    expect(runs).toContain('cargo fmt --all --check');
    expect(runs).toContain('cargo clippy --workspace --all-targets -- -D warnings');
    expect(runs).toContain('cargo test --workspace');
  });

  test('builds the arm64 Lambda with the pinned cargo-lambda', () => {
    const runs = checks.wf.jobs.lambda.steps.map((s) => s.run ?? '');
    expect(runs).toContain('make build-lambdas');
    expect(runs.some((r) => r.includes('cargo-lambda==${CARGO_LAMBDA_VERSION}'))).toBe(true);
    expect(checks.wf.env?.CARGO_LAMBDA_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });

  test('uses the same tool pins as the deploy workflow', () => {
    expect(checks.wf.env?.CARGO_LAMBDA_VERSION).toBe(deploy.wf.env?.CARGO_LAMBDA_VERSION);
    expect(checks.wf.env?.ZIGLANG_VERSION).toBe(deploy.wf.env?.ZIGLANG_VERSION);
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

  test('checks the shell scripts', () => {
    const runs = checks.wf.jobs.scripts.steps.map((s) => s.run ?? '').join('\n');
    expect(runs).toContain('bash -n');
    expect(runs).toContain('shellcheck -x');
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
  const { build, deploy: deployJob, checks: checksJob } = deploy.wf.jobs as unknown as Record<string, Job & { uses?: string; needs?: string }>;
  const deploySteps = deployJob.steps;
  const deployRuns = deploySteps.map((s) => s.run ?? '');
  const names = deploySteps.map((s) => s.name);

  test('runs on pushes to main and manual dispatch only', () => {
    expect(Object.keys(deploy.wf.on).sort()).toEqual(['push', 'workflow_dispatch']);
    expect(deploy.wf.on.push).toEqual({ branches: ['main'] });
  });

  test('never runs two deployments at once and never cancels one', () => {
    expect(deploy.wf.concurrency).toEqual({ group: 'deploy-production', 'cancel-in-progress': false });
  });

  test('runs the checks workflow first, on the same commit, and the other jobs need it', () => {
    expect(Object.keys(deploy.wf.jobs)).toEqual(['checks', 'build', 'deploy']);
    expect(checksJob.uses).toBe('./.github/workflows/checks.yml');
    expect(build.needs).toBe('checks');
    expect(deployJob.needs).toBe('build');
  });

  test('only the deploy job gets an OIDC token and the production environment', () => {
    expect(deployJob.environment).toBe('production');
    expect(deployJob.permissions).toEqual({ contents: 'read', 'id-token': 'write' });
    expect(build.permissions).toEqual({ contents: 'read' });
    expect(checksJob.permissions).toEqual({ contents: 'read' });
    expect(build.environment).toBeUndefined();
    expect(deploy.wf.permissions).toEqual({ contents: 'read' });
  });

  test('the build job refuses any ref but main, builds, tests, and synthesizes prod with no AWS identity', () => {
    expect(build.steps[0].name).toBe('require main');
    expect(deploy.text).toContain('refs/heads/main');
    const runs = build.steps.map((s) => s.run ?? '').join('\n');
    expect(runs).toContain('cargo lambda build --release --arm64 -p openbook-api');
    expect(runs).toContain('npm run build');
    expect(runs).toContain('npm test');
    expect(runs).toContain('npx cdk synth --quiet -c stage=prod');
    expect(build.steps.some((s) => s.uses?.startsWith('aws-actions/'))).toBe(false);
    expect(deploy.wf.env?.CARGO_LAMBDA_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
    const upload = build.steps.find((s) => s.uses?.startsWith('actions/upload-artifact@'));
    expect(upload?.with).toMatchObject({ name: 'cdk-out', path: 'infra/cdk.out' });
  });

  test('the deploy job builds and tests nothing: it downloads the artifact and installs without scripts', () => {
    const all = deployRuns.join('\n');
    expect(all).not.toMatch(/\bcargo\b|pip3?\b|npm test|npm run|\bnpx (vite|vitest|tsx|tsc)\b/);
    expect(deployRuns).toContain('npm ci --ignore-scripts');
    expect(deployRuns.filter((r) => /^npm ci\b/.test(r) && !r.includes('--ignore-scripts'))).toEqual([]);
    const download = deploySteps.find((s) => s.uses?.startsWith('actions/download-artifact@'));
    expect(download?.with).toMatchObject({ name: 'cdk-out', path: 'infra/cdk.out' });
    // The install and the download come before the credentials, and the deploy after.
    const assume = names.indexOf('assume the deployment role');
    expect(deployRuns.indexOf('npm ci --ignore-scripts')).toBeLessThan(assume);
    expect(deploySteps.indexOf(download as Step)).toBeLessThan(assume);
    expect(names.indexOf('deploy')).toBeGreaterThan(assume);
  });

  test('assumes the role from a secret through OIDC, masks the account, and pins the account', () => {
    const step = deploySteps.find((s) => s.uses?.startsWith('aws-actions/configure-aws-credentials@'));
    expect(step?.with).toMatchObject({
      'role-to-assume': '${{ secrets.AWS_DEPLOY_ROLE_ARN }}',
      'aws-region': 'ap-southeast-2',
      audience: 'sts.amazonaws.com',
      'mask-aws-account-id': true,
      'allowed-account-ids': '${{ secrets.AWS_ACCOUNT_ID }}',
    });
  });

  test('reads every account value from an environment secret, never a variable', () => {
    expect(deploy.text).not.toMatch(/\bvars\./);
    const secrets = new Set([...deploy.text.matchAll(/secrets\.([A-Z_]+)/g)].map((m) => m[1]));
    expect([...secrets].sort()).toEqual(['AWS_ACCOUNT_ID', 'AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN', 'AWS_DEPLOY_ROLE_ARN']);
    expect(deploy.text).not.toMatch(/arn:aws:iam::\d/);
  });

  test('checks its secrets before it installs or deploys anything', () => {
    expect(names[0]).toBe('require the deployment configuration');
  });

  test('deploys only Openbook-prod, as the execution role, from the artifact, without prompts', () => {
    const step = deploySteps.find((s) => s.name === 'deploy');
    expect(step?.run).toContain('npx cdk deploy Openbook-prod');
    expect(step?.run).toContain('--app cdk.out');
    expect(step?.run).toContain('--require-approval never');
    expect(step?.run).toContain('--role-arn "$AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN"');
    expect(step?.env?.AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN).toBe('${{ secrets.AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN }}');
    expect(step?.run).not.toMatch(/--all|Openbook-dev/);
  });

  test('sends the cdk deploy output to a file and prints only a filtered summary', () => {
    const step = deploySteps.find((s) => s.name === 'deploy');
    expect(step?.run).toContain('> "$log" 2>&1');
    expect(step?.run).toContain('--outputs-file cdk-outputs.json');
    // Every line that reaches the log goes through the mask, which removes account numbers and ARNs.
    expect(step?.run).toContain("mask() { sed -E 's/[0-9]{12}/<account>/g;");
    const echoes = (step?.run ?? '').split('\n').filter((l) => /\bgrep\b/.test(l));
    for (const line of echoes) expect(line, line).toContain('mask');
  });

  test('smoke tests /api/health for ok: true and / for 200 after the deploy', () => {
    const deployAt = names.indexOf('deploy');
    const health = deploySteps.find((s) => s.name === 'smoke test /api/health');
    const root = deploySteps.find((s) => s.name === 'smoke test /');
    expect(health?.run).toContain('/api/health');
    expect(health?.run).toContain('.ok == true');
    expect(root?.run).toContain('!= 200');
    expect(names.indexOf('smoke test /api/health')).toBeGreaterThan(deployAt);
    expect(names.indexOf('smoke test /')).toBeGreaterThan(deployAt);
  });
});

describe('the gitignore', () => {
  test('ignores the deploy outputs file and the rendered IAM documents', () => {
    const ignore = read('.gitignore');
    expect(ignore).toContain('cdk-outputs.json');
    expect(ignore).toContain('/infra/iam/rendered/');
  });
});
