import { spawnSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { parse } from 'yaml';
import { afterAll, describe, expect, test } from 'vitest';
import { certificateArnPattern, certificateIdPattern, hostedZoneIdPattern } from '../lib/deployment';
import { copyAgnosticAssembly } from './helpers';

const root = path.resolve(__dirname, '..', '..');
const read = (file: string): string => fs.readFileSync(path.join(root, file), 'utf8');

interface Step {
  id?: string;
  name?: string;
  uses?: string;
  run?: string;
  env?: Record<string, string>;
  with?: Record<string, string | boolean>;
  'working-directory'?: string;
}
interface Job {
  outputs?: Record<string, string>;
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
      'eval',
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
    expect(deploy.wf.on.push).toMatchObject({ branches: ['main'] });
  });

  test('a push that changes only docs, Markdown, evaluation results, or the licence skips the whole pipeline', () => {
    const push = deploy.wf.on.push as { branches: string[]; 'paths-ignore'?: string[]; paths?: string[] };
    expect(push['paths-ignore']).toEqual(['docs/**', '**/*.md', 'evals/results/**', 'LICENSE']);
    // `paths` and `paths-ignore` cannot be mixed, and a manual run must stay possible.
    expect(push.paths).toBeUndefined();
    expect(Object.keys(deploy.wf.on)).toContain('workflow_dispatch');
    // The ignore list names no code, workflow, script, infrastructure, or evaluation input.
    for (const pattern of push['paths-ignore'] ?? []) {
      expect(pattern).not.toMatch(/^(crates|apps|infra|scripts|\.github|evals\/(corpora|baseline))/);
    }
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
    expect(runs).toContain('make build-lambdas');
    expect(runs).toContain('npm run build');
    expect(runs).toContain('npm test');
    expect(runs).toContain('npx cdk synth --quiet -c stage=prod');
    expect(build.steps.some((s) => s.uses?.startsWith('aws-actions/'))).toBe(false);
    expect(deploy.wf.env?.CARGO_LAMBDA_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
    // The assembly travels as one tar, because the artifact actions save every file as 644
    // and the Lambda bootstrap must stay executable. The tar's hash is a job output.
    const pack = build.steps.find((s) => s.name === 'pack the cloud assembly');
    expect(pack?.run).toContain('tar -C infra -cf cdk-assembly.tar cdk.out');
    expect(pack?.run).toContain('sha256sum cdk-assembly.tar');
    expect(pack?.id).toBe('pack');
    expect(build.outputs).toEqual({ assembly_sha256: '${{ steps.pack.outputs.sha256 }}' });
    const upload = build.steps.find((s) => s.uses?.startsWith('actions/upload-artifact@'));
    expect(upload?.with).toMatchObject({ name: 'cdk-assembly', path: 'cdk-assembly.tar' });
    expect(build.steps.indexOf(pack as Step)).toBeLessThan(build.steps.indexOf(upload as Step));
  });

  test('the deploy job builds and tests nothing: it downloads the artifact and installs without scripts', () => {
    const all = deployRuns.join('\n');
    expect(all).not.toMatch(/\bcargo\b|pip3?\b|npm test|npm run|\bnpx (vite|vitest|tsx|tsc)\b/);
    expect(deployRuns).toContain('npm ci --ignore-scripts');
    expect(deployRuns.filter((r) => /^npm ci\b/.test(r) && !r.includes('--ignore-scripts'))).toEqual([]);
    const download = deploySteps.find((s) => s.uses?.startsWith('actions/download-artifact@'));
    expect(download?.with).toMatchObject({ name: 'cdk-assembly', path: 'assembly' });
    // The install, the download, and the validation come before the credentials, and the
    // deploy after. A refused assembly never meets a token.
    const assume = names.indexOf('assume the deployment role');
    const validate = names.indexOf('verify, unpack, and validate the cloud assembly');
    expect(deployRuns.indexOf('npm ci --ignore-scripts')).toBeLessThan(assume);
    expect(deploySteps.indexOf(download as Step)).toBeLessThan(validate);
    expect(validate).toBeLessThan(assume);
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
    expect([...secrets].sort()).toEqual([
      'AWS_ACCOUNT_ID',
      'AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN',
      'AWS_DEPLOY_ROLE_ARN',
      'OPENBOOK_CERTIFICATE_ARN',
      'OPENBOOK_HOSTED_ZONE_ID',
    ]);
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

  test('passes the certificate ARN and the hosted zone ID as parameters of Openbook-prod, from secrets', () => {
    const step = deploySteps.find((s) => s.name === 'deploy');
    expect(step?.run).toContain('--parameters "Openbook-prod:CertificateArn=$OPENBOOK_CERTIFICATE_ARN"');
    expect(step?.run).toContain('--parameters "Openbook-prod:HostedZoneId=$OPENBOOK_HOSTED_ZONE_ID"');
    expect(step?.env?.OPENBOOK_CERTIFICATE_ARN).toBe('${{ secrets.OPENBOOK_CERTIFICATE_ARN }}');
    expect(step?.env?.OPENBOOK_HOSTED_ZONE_ID).toBe('${{ secrets.OPENBOOK_HOSTED_ZONE_ID }}');
    // The build job has no secret, so the synthesized template cannot hold either value.
    expect((build.steps.map((x) => JSON.stringify(x))).join('')).not.toMatch(/secrets\./);
  });

  test('checks the two secrets with the patterns of the stack parameters', () => {
    const step = deploySteps.find((s) => s.name === 'require the deployment configuration');
    // One definition each, in lib/deployment.ts. The bash copy swaps the account for the secret.
    expect(step?.run).toContain(hostedZoneIdPattern);
    expect(step?.run).toContain(`certificate/${certificateIdPattern}$`);
    expect(certificateArnPattern).toContain(`certificate/${certificateIdPattern}$`);
    expect(certificateArnPattern).toContain('arn:aws:acm:us-east-1:');
    expect(step?.run).toContain('arn:aws:acm:us-east-1:${AWS_ACCOUNT_ID}:certificate/');
  });

  test('masks a hosted zone ID in the deploy log', () => {
    const step = deploySteps.find((s) => s.name === 'deploy');
    expect(step?.run).toContain('<zone>');
  });

  test('reads SiteUrl, and requires the custom domain of cdk.json when there is one', () => {
    const step = deploySteps.find((s) => s.name === 'read the site URL');
    expect(step?.run).toContain('.context.stageDomains.prod.domainName');
    expect(step?.run).toContain('"https://$domain"');
    const cdkJson = JSON.parse(read('infra/cdk.json')) as { context: { stageDomains: { prod: { domainName: string } } } };
    // The workflow reads the domain from cdk.json, so a change of the domain needs no change of the workflow.
    expect(cdkJson.context.stageDomains.prod.domainName).toBe('openbook.steve.gg');
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
    expect(root?.run).toContain('= 200');
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

describe('the verify, unpack, and validate step', () => {
  const step = (deploy.wf.jobs.deploy as Job).steps.find((x) => x.name === 'verify, unpack, and validate the cloud assembly');
  const root = path.resolve(__dirname, '..', '..');
  const work = fs.mkdtempSync(path.join(os.tmpdir(), 'openbook-verify-'));
  let counter = 0;

  afterAll(() => fs.rmSync(work, { recursive: true, force: true }));

  // The step runs on Ubuntu, which has sha256sum. macOS has shasum only, so give the test a shim.
  const bin = path.join(work, 'bin');
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, 'sha256sum'), '#!/bin/sh\nexec shasum -a 256 "$@"\n', { mode: 0o755 });

  const sha = (file: string): string =>
    spawnSync('shasum', ['-a', '256', file], { encoding: 'utf8' }).stdout.split(' ')[0];

  /** Lays out a runner folder, runs the step body in it, and returns the result. */
  function runStep(options: {
    prepare?: (cdkOut: string, parent: string) => void;
    tarOf?: (parent: string, tar: string) => void;
    expectedHash?: (actual: string) => string;
  }): { status: number | null; output: string; dir: string } {
    counter += 1;
    const dir = path.join(work, `run-${counter}`);
    const parent = path.join(dir, 'build', 'infra');
    fs.mkdirSync(path.join(dir, 'infra', 'scripts'), { recursive: true });
    fs.mkdirSync(path.join(dir, 'assembly'), { recursive: true });
    fs.copyFileSync(
      path.join(root, 'infra', 'scripts', 'validate-assembly.js'),
      path.join(dir, 'infra', 'scripts', 'validate-assembly.js'),
    );
    const cdkOut = path.join(parent, 'cdk.out');
    copyAgnosticAssembly(cdkOut);
    options.prepare?.(cdkOut, parent);
    const tar = path.join(dir, 'assembly', 'cdk-assembly.tar');
    if (options.tarOf) options.tarOf(parent, tar);
    else spawnSync('tar', ['-C', parent, '-cf', tar, 'cdk.out']);
    const expected = options.expectedHash ? options.expectedHash(sha(tar)) : sha(tar);
    const result = spawnSync('bash', ['-e', '-c', step?.run ?? ''], {
      cwd: dir,
      encoding: 'utf8',
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, EXPECTED_SHA256: expected },
    });
    return { status: result.status, output: result.stdout + result.stderr, dir };
  }

  const bootstrapOf = (cdkOut: string): string => {
    const hit = fs.readdirSync(cdkOut).find((d) => fs.existsSync(path.join(cdkOut, d, 'bootstrap')));
    return path.join(cdkOut, hit ?? 'none', 'bootstrap');
  };

  test('the fixture Lambda binary is executable in the repository', () => {
    // If git stored it as 644, every other check in this describe would prove nothing.
    expect(fs.statSync(path.join(root, 'infra', 'test', 'fixtures', 'api', 'bootstrap')).mode & 0o111).not.toBe(0);
  });

  test('accepts the assembly that the build job packed, and the unpacked bootstrap is still executable', () => {
    const result = runStep({});
    expect(result.output).toContain('expected shape');
    expect(result.status).toBe(0);
    const unpacked = bootstrapOf(path.join(result.dir, 'infra', 'cdk.out'));
    expect(fs.statSync(unpacked).mode & 0o111).not.toBe(0);
  });

  test('refuses a bootstrap that lost its execute bit, as the artifact actions would make it', () => {
    const result = runStep({ prepare: (cdkOut) => fs.chmodSync(bootstrapOf(cdkOut), 0o644) });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('not executable');
  });

  test('refuses an assembly without libpdfium.so beside the ingest bootstrap', () => {
    const result = runStep({
      prepare: (cdkOut) => {
        for (const dir of fs.readdirSync(cdkOut)) {
          if (fs.statSync(path.join(cdkOut, dir)).isDirectory()) fs.rmSync(path.join(cdkOut, dir, 'libpdfium.so'), { force: true });
        }
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('libpdfium.so is missing');
  });

  test('refuses an assembly with only one Lambda binary', () => {
    const result = runStep({
      prepare: (cdkOut) => {
        const dirs = fs.readdirSync(cdkOut).filter((d) => fs.existsSync(path.join(cdkOut, d, 'bootstrap')));
        fs.rmSync(path.join(cdkOut, dirs[0], 'bootstrap'));
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('exactly two Lambda bootstrap binaries');
  });

  test('refuses a tar whose hash is not the one that the build job recorded', () => {
    const result = runStep({ expectedHash: (actual) => actual.replace(/^./, (c) => (c === '0' ? '1' : '0')) });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('not the one that the build job packed');
  });

  test('refuses an empty recorded hash', () => {
    const result = runStep({ expectedHash: () => '' });
    expect(result.status).not.toBe(0);
  });

  test('refuses a tar with an entry outside cdk.out', () => {
    const result = runStep({
      prepare: (_cdkOut, parent) => fs.writeFileSync(path.join(parent, 'evil'), 'x'),
      tarOf: (parent, tar) => spawnSync('tar', ['-C', parent, '-cf', tar, 'cdk.out', 'evil']),
    });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('outside cdk.out');
  });

  test('refuses a tar with a symbolic link', () => {
    const result = runStep({
      prepare: (cdkOut) => fs.symlinkSync('/etc/passwd', path.join(cdkOut, 'link')),
    });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('not a plain file or directory');
  });

  test('refuses an assembly that has an executable asset, even when the hash and the tar are fine', () => {
    const result = runStep({
      prepare: (cdkOut) => {
        const file = path.join(cdkOut, 'Openbook-prod.assets.json');
        const assets = JSON.parse(fs.readFileSync(file, 'utf8')) as { files: Record<string, { source: Record<string, unknown> }> };
        const first = Object.values(assets.files)[0];
        first.source = { executable: ['sh', '-c', 'echo pwned'], packaging: 'file' };
        fs.writeFileSync(file, JSON.stringify(assets));
      },
    });
    expect(result.status).not.toBe(0);
    expect(result.output).toContain('cloud assembly refused');
  });
});

describe('the phase 2 packaging', () => {
  test('make build-lambdas builds both binaries for arm64 and puts libpdfium.so next to the worker', () => {
    expect(read('Makefile')).toMatch(/build-lambdas:.*\n\tscripts\/build-lambdas\.sh/);
    const script = read('scripts/build-lambdas.sh');
    expect(script).toContain('cargo lambda build --release --arm64 -p openbook-api -p openbook-ingest');
    expect(script).toContain('pdfium_fetch linux-arm64');
    expect(script).toContain('target/lambda/openbook-ingest/libpdfium.so');
  });

  test('the deploy build job uses make build-lambdas, and the checks build the same way', () => {
    const buildRuns = (deploy.wf.jobs.build as Job).steps.map((s) => s.run ?? '');
    expect(buildRuns).toContain('make build-lambdas');
    expect(checks.wf.jobs.lambda.steps.map((s) => s.run)).toContain('make build-lambdas');
    const lambdaRun = checks.wf.jobs.lambda.steps.map((s) => s.run ?? '').join('\n');
    expect(lambdaRun).toContain('target/lambda/openbook-ingest/libpdfium.so');
  });

  test('the Rust test jobs fetch pdfium for linux-x64 and export its path', () => {
    for (const job of ['rust', 'rust-integration']) {
      const runs = checks.wf.jobs[job].steps.map((s) => s.run ?? '').join('\n');
      expect(runs, job).toContain('OPENBOOK_PDFIUM_PATH=$(scripts/fetch-pdfium.sh linux-x64)');
      expect(runs, job).toContain('>> "$GITHUB_ENV"');
    }
  });
});

describe('the retrieval evaluation', () => {
  test('CI runs it offline, with no AWS and no secret', () => {
    const job = checks.wf.jobs.eval;
    expect(job.steps.map((s) => s.run)).toContain('make eval');
    expect(JSON.stringify(job)).not.toMatch(/secrets\.|vars\.|aws-actions|id-token/);
  });

  test('make eval is the offline check, and make eval-live needs AWS_PROFILE, says that it calls Bedrock, and starts only with YES=1', () => {
    const makefile = read('Makefile');
    // The help line and the recipe of one target: from `name:` to the first empty line.
    const target = (name: string): string => makefile.match(new RegExp(`^${name}:.*\\n(?:\\t.*\\n)*`, 'm'))?.[0] ?? '';
    expect(target('eval')).toMatch(/\n\tcargo run .*-p openbook-eval -- offline --check\n/);
    const live = target('eval-live');
    expect(live).toMatch(/^eval-live:.*Bedrock/);
    expect(live).toMatch(/\n\t@test -n "\$\$\{AWS_PROFILE:-\}" \|\| \{ .*exit 1; \}\n/);
    expect(live).toMatch(/openbook-eval -- refresh \$\(if \$\(YES\),--yes,\)\n/);
    // The incremental refresh has the same safety shape: it names Bedrock, needs a profile, and
    // starts only with YES=1.
    const missing = target('eval-live-missing');
    expect(missing).toMatch(/^eval-live-missing:.*Bedrock/);
    expect(missing).toMatch(/\n\t@test -n "\$\$\{AWS_PROFILE:-\}" \|\| \{ .*exit 1; \}\n/);
    expect(missing).toMatch(/openbook-eval -- refresh --missing-only \$\(if \$\(YES\),--yes,\)\n/);
    // The grid is offline: no profile, no Bedrock.
    expect(target('eval-grid')).toMatch(/openbook-eval -- grid\n/);
    expect(target('eval-grid')).not.toMatch(/AWS|Bedrock/);
  });
});

describe('make deploy and the custom domain', () => {
  const root = path.resolve(__dirname, '..', '..');
  const make = (args: string[], env: Record<string, string | undefined>): { status: number | null; output: string } => {
    const result = spawnSync('make', args, {
      cwd: root,
      encoding: 'utf8',
      env: { PATH: process.env.PATH ?? '', HOME: os.tmpdir(), ...env },
    });
    return { status: result.status, output: result.stdout + result.stderr };
  };
  const prod = ['deploy', 'STAGE=prod', 'ALLOW_PROD=1'];
  const profile = { AWS_PROFILE: 'x' };
  const vars = { OPENBOOK_CERTIFICATE_ARN: 'cert-arn', OPENBOOK_HOSTED_ZONE_ID: 'ZONEID' };

  test('prod has a domain in cdk.json, so it refuses to start without both variables', () => {
    for (const env of [{}, { OPENBOOK_CERTIFICATE_ARN: 'x' }, { OPENBOOK_HOSTED_ZONE_ID: 'x' }]) {
      const result = make(prod, { ...profile, ...env });
      expect(result.status).not.toBe(0);
      expect(result.output).toContain('openbook.steve.gg');
      expect(result.output).toContain('OPENBOOK_CERTIFICATE_ARN');
    }
  });

  test('passes both variables as the parameters of the stage stack', () => {
    const result = make([...prod, '-n'], { ...profile, ...vars });
    expect(result.status).toBe(0);
    expect(result.output).toContain('--parameters "Openbook-prod:CertificateArn=$OPENBOOK_CERTIFICATE_ARN"');
    expect(result.output).toContain('--parameters "Openbook-prod:HostedZoneId=$OPENBOOK_HOSTED_ZONE_ID"');
  });

  test('dev has no domain: it needs no variable and passes no parameter', () => {
    const result = make(['deploy', 'STAGE=dev', '-n'], profile);
    expect(result.status).toBe(0);
    expect(result.output).not.toContain('--parameters');
  });
});

describe('the require the deployment configuration step', () => {
  const step = (deploy.wf.jobs.deploy as Job).steps.find((x) => x.name === 'require the deployment configuration');
  const account = '123456789012';
  const good: Record<string, string> = {
    AWS_ACCOUNT_ID: account,
    AWS_DEPLOY_ROLE_ARN: `arn:aws:iam::${account}:role/deploy`,
    AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN: `arn:aws:iam::${account}:role/exec`,
    OPENBOOK_CERTIFICATE_ARN: `arn:aws:acm:us-east-1:${account}:certificate/12345678-abcd-4321-abcd-123456789012`,
    OPENBOOK_HOSTED_ZONE_ID: 'Z0123456789ABCDEFGHIJ',
  };

  const run = (override: Record<string, string | undefined>): { status: number | null; output: string } => {
    const env: Record<string, string> = { PATH: process.env.PATH ?? '', HOME: os.tmpdir(), ...good };
    for (const [k, v] of Object.entries(override)) {
      if (v === undefined) delete env[k];
      else env[k] = v;
    }
    const result = spawnSync('bash', ['-e', '-c', step?.run ?? ''], { encoding: 'utf8', env });
    return { status: result.status, output: result.stdout + result.stderr };
  };

  test('declares both new secrets in its environment', () => {
    expect(step?.env?.OPENBOOK_CERTIFICATE_ARN).toBe('${{ secrets.OPENBOOK_CERTIFICATE_ARN }}');
    expect(step?.env?.OPENBOOK_HOSTED_ZONE_ID).toBe('${{ secrets.OPENBOOK_HOSTED_ZONE_ID }}');
  });

  test('accepts a consistent configuration', () => {
    expect(run({}).status).toBe(0);
  });

  test.each(['OPENBOOK_CERTIFICATE_ARN', 'OPENBOOK_HOSTED_ZONE_ID'])('refuses a missing %s, and names it', (name) => {
    for (const missing of [undefined, '']) {
      const result = run({ [name]: missing });
      expect(result.status).toBe(1);
      expect(result.output).toContain(name);
    }
  });

  test.each([
    ['a certificate in another Region', `arn:aws:acm:ap-southeast-2:${account}:certificate/12345678-abcd-4321-abcd-123456789012`],
    ['a certificate of another account', 'arn:aws:acm:us-east-1:999999999999:certificate/12345678-abcd-4321-abcd-123456789012'],
    ['a role ARN', `arn:aws:iam::${account}:role/exec`],
    ['a certificate with a suffix', `arn:aws:acm:us-east-1:${account}:certificate/12345678-abcd-4321-abcd-123456789012/x`],
    ['a certificate ID, not an ARN', '12345678-abcd-4321-abcd-123456789012'],
  ])('refuses %s as OPENBOOK_CERTIFICATE_ARN, and prints no value', (_what, value) => {
    const result = run({ OPENBOOK_CERTIFICATE_ARN: value });
    expect(result.status).toBe(1);
    expect(result.output).toContain('OPENBOOK_CERTIFICATE_ARN');
    expect(result.output).not.toContain(value);
  });

  test.each(['z0123456789', '/hostedzone/Z0123456789', 'Z!', 'Z012345678901234567890123456789012', 'Z0123 456', 'Z01\nX'])(
    'refuses %j as OPENBOOK_HOSTED_ZONE_ID, and prints no value',
    (value) => {
      const result = run({ OPENBOOK_HOSTED_ZONE_ID: value });
      expect(result.status).toBe(1);
      expect(result.output).toContain('OPENBOOK_HOSTED_ZONE_ID');
      expect(result.output).not.toContain(value);
    },
  );

  test('never prints a value of a good configuration', () => {
    const result = run({});
    for (const value of Object.values(good)) expect(result.output).not.toContain(value);
  });
});
