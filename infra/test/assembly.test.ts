import { spawnSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';
import { copyAgnosticAssembly, type Json } from './helpers';

/**
 * The deploy job runs the CDK CLI on an assembly that the build job made. The CLI would run
 * `source.executable` of an asset, so `scripts/validate-assembly.js` accepts only the shape
 * of the stack today. These tests run it on the real assembly and on tampered copies.
 */

const script = path.resolve(__dirname, '..', 'scripts', 'validate-assembly.js');
const work = fs.mkdtempSync(path.join(os.tmpdir(), 'openbook-assembly-'));
let base = '';
afterAll(() => fs.rmSync(work, { recursive: true, force: true }));

beforeAll(() => {
  base = path.join(work, 'base');
  copyAgnosticAssembly(base);
});

const run = (dir: string): { status: number | null; stderr: string; stdout: string } => {
  const result = spawnSync('node', [script, dir], { encoding: 'utf8' });
  return { status: result.status, stderr: result.stderr, stdout: result.stdout };
};

let counter = 0;
/** Copies the base assembly, lets the test change it, and returns the validator result. */
function tampered(change: (dir: string, manifest: Json, assets: Json) => void): ReturnType<typeof run> {
  counter += 1;
  const dir = path.join(work, `copy-${counter}`);
  fs.cpSync(base, dir, { recursive: true });
  const manifestPath = path.join(dir, 'manifest.json');
  const assetsPath = path.join(dir, 'Openbook-prod.assets.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8')) as Json;
  const assets = JSON.parse(fs.readFileSync(assetsPath, 'utf8')) as Json;
  change(dir, manifest, assets);
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  fs.writeFileSync(assetsPath, JSON.stringify(assets));
  return run(dir);
}

type Files = Record<string, { source: Json; destinations: Record<string, Json> }>;
const firstFile = (assets: Json): Files[string] => Object.values(assets.files as Files)[0];
const stackOf = (manifest: Json): Json => (manifest.artifacts as Record<string, Json>)['Openbook-prod'];

describe('the assembly validator', () => {
  test('accepts the real account-agnostic assembly', () => {
    const result = run(base);
    expect(result.stderr).toBe('');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('expected shape');
  });

  test('refuses an asset that has an executable, the way the CDK CLI would run it', () => {
    const result = tampered((_dir, _m, assets) => {
      firstFile(assets).source = { executable: ['sh', '-c', 'echo pwned'], packaging: 'file' };
    });
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/executable|not allowed/);
  });

  test('refuses an executable that sits next to a path', () => {
    const result = tampered((_dir, _m, assets) => {
      firstFile(assets).source = { ...firstFile(assets).source, executable: ['sh', '-c', 'true'] };
    });
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('"executable"');
  });

  test.each([
    ['a role to assume in the stack artifact', (m: Json) => (stackOf(m).assumeRoleArn = 'arn:aws:iam::1:role/x')],
    ['an execution role in the stack artifact', (m: Json) => ((stackOf(m).properties as Json).cloudFormationExecutionRoleArn = 'arn:x')],
    ['a lookup role in the stack artifact', (m: Json) => ((stackOf(m).properties as Json).lookupRole = { arn: 'arn:x' })],
    ['a concrete account', (m: Json) => (stackOf(m).environment = 'aws://123456789012/ap-southeast-2')],
    ['another Region', (m: Json) => (stackOf(m).environment = 'aws://unknown-account/us-east-1')],
    ['an unknown artifact', (m: Json) => ((m.artifacts as Json).Extra = { type: 'cdk:asset-manifest', properties: { file: 'x.json' } })],
    ['a second stack', (m: Json) => ((m.artifacts as Json)['Openbook-dev'] = { type: 'aws:cloudformation:stack' })],
    ['a stack template URL in another bucket', (m: Json) => ((stackOf(m).properties as Json).stackTemplateAssetObjectUrl = 's3://evil/openbook-prod/' + 'a'.repeat(64) + '.json')],
    ['another template file', (m: Json) => ((stackOf(m).properties as Json).templateFile = 'other.template.json')],
  ])('refuses %s', (_name, change) => {
    const result = tampered((_dir, manifest) => change(manifest));
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('cloud assembly refused');
  });

  test.each([
    ['a docker image', (a: Json) => (a.dockerImages = { x: { source: { directory: '.' }, destinations: {} } })],
    ['an assume role in a destination', (a: Json) => (Object.values(firstFile(a).destinations)[0].assumeRoleArn = 'arn:aws:iam::1:role/x')],
    ['another bucket', (a: Json) => (Object.values(firstFile(a).destinations)[0].bucketName = 'cdk-hnb659fds-assets-123456789012-ap-southeast-2')],
    ['another prefix', (a: Json) => (Object.values(firstFile(a).destinations)[0].objectKey = 'other-app/' + 'a'.repeat(64) + '.zip')],
    ['no prefix', (a: Json) => (Object.values(firstFile(a).destinations)[0].objectKey = 'a'.repeat(64) + '.zip')],
    ['another Region', (a: Json) => (Object.values(firstFile(a).destinations)[0].region = 'us-east-1')],
    ['an unknown packaging', (a: Json) => (firstFile(a).source.packaging = 'exec')],
    ['an extra source field', (a: Json) => (firstFile(a).source.env = { A: 'b' })],
    ['an extra file field', (a: Json) => (firstFile(a).source.path = firstFile(a).source.path, ((firstFile(a) as Json).hook = 'x'))],
    ['a parent path', (a: Json) => (firstFile(a).source.path = '../outside')],
    ['an absolute path', (a: Json) => (firstFile(a).source.path = '/etc')],
    ['a path inside that climbs out', (a: Json) => (firstFile(a).source.path = 'asset.x/../../outside')],
    ['a backslash path', (a: Json) => (firstFile(a).source.path = '..\\outside')],
    ['a missing path', (a: Json) => (firstFile(a).source.path = 'asset.' + '0'.repeat(64))],
    ['a key that is not a hash', (a: Json) => {
      const files = a.files as Files;
      files['not-a-hash'] = files[Object.keys(files)[0]];
      delete files[Object.keys(files)[0]];
    }],
    ['no files', (a: Json) => (a.files = {})],
  ])('refuses %s', (_name, change) => {
    const result = tampered((_dir, _manifest, assets) => change(assets));
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('cloud assembly refused');
  });

  test('accepts the ingest worker zip, and refuses one asset more or less', () => {
    const names = (assets: Json): string[] => Object.values(assets.files as Record<string, { displayName: string }>).map((f) => f.displayName);
    let seen: string[] = [];
    expect(tampered((_d, _m, assets) => { seen = names(assets); }).status).toBe(0);
    expect(seen).toContain('IngestFunction/Code');
    expect(seen).toHaveLength(8);
    // One more asset: a copy of the API asset under another name and key.
    const more = tampered((_d, _m, assets) => {
      const files = assets.files as Record<string, { displayName: string }>;
      const [hash, file] = Object.entries(files)[0];
      files['b'.repeat(64)] = { ...JSON.parse(JSON.stringify(file)), displayName: 'EvilFunction/Code' };
      void hash;
    });
    expect(more.status).toBe(1);
    expect(more.stderr).toContain('not an asset of the stack');
    // One less: the ingest zip removed.
    const less = tampered((_d, _m, assets) => {
      const files = assets.files as Record<string, { displayName: string }>;
      for (const [hash, file] of Object.entries(files)) if (file.displayName === 'IngestFunction/Code') delete files[hash];
    });
    expect(less.status).toBe(1);
    expect(less.stderr).toContain('"IngestFunction/Code" is missing');
  });

  test('refuses an asset that changes its packaging or appears twice', () => {
    const packaging = tampered((_d, _m, assets) => {
      const f = Object.values(assets.files as Record<string, { displayName: string; source: Json }>).find((x) => x.displayName === 'IngestFunction/Code');
      if (f) f.source.packaging = 'file';
    });
    expect(packaging.status).toBe(1);
    expect(packaging.stderr).toMatch(/packaging|directory/);
  });

  test('refuses a template asset that is not the hashed template file', () => {
    const result = tampered((dir) => {
      const template = path.join(dir, 'Openbook-prod.template.json');
      fs.writeFileSync(template, fs.readFileSync(template, 'utf8') + ' ');
    });
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/hash of the template/);
  });

  test('refuses a symbolic link anywhere in the assembly', () => {
    const result = tampered((dir) => {
      fs.symlinkSync('/etc/passwd', path.join(dir, 'asset-link'));
    });
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('symbolic link');
  });

  test('refuses a directory that has no manifest, and the wrong number of arguments', () => {
    expect(run(path.join(work, 'nowhere')).status).toBe(1);
    expect(spawnSync('node', [script], { encoding: 'utf8' }).status).toBe(2);
  });
});
