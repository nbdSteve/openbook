import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, test } from 'vitest';
import {
  assertIgnoredOutDir,
  defaultOutDir,
  exampleValues,
  iamDir,
  parseArgv,
  placeholdersIn,
  readTemplate,
  renderAll,
  renderIamDocument,
  templateNames,
} from '../scripts/render-iam';

const values = exampleValues();
const secret = 'nbdSteve@123456789/openbook@987654321';

describe('the example values', () => {
  test('supply every placeholder the templates use, and nothing else', () => {
    expect(() => renderAll(values)).not.toThrow();
    expect(renderAll(values).size).toBe(templateNames().length);
  });

  test('name an account nobody owns and a synthetic repository identity', () => {
    expect(values.ACCOUNT_ID).toBe('000000000000');
    expect(values.GITHUB_REPOSITORY_IDENTITY).toBe('nbdSteve@1/openbook@2');
  });
});

describe('rendering', () => {
  const template = readTemplate('github-deploy-role-policy');

  test('substitutes every placeholder', () => {
    const document = renderIamDocument(template, values);
    expect(placeholdersIn(template).size).toBeGreaterThan(0);
    expect(placeholdersIn(document).size).toBe(0);
  });

  test('is deterministic and canonical', () => {
    const a = renderIamDocument(template, values);
    expect(renderIamDocument(template, values)).toBe(a);
    expect(a.endsWith('\n')).toBe(true);
    expect(a).toBe(`${JSON.stringify(JSON.parse(a), null, 2)}\n`);
  });

  test('refuses a placeholder it does not know', () => {
    expect(() => renderIamDocument('{"A": "${NOT_A_PLACEHOLDER}"}', values)).toThrow('NOT_A_PLACEHOLDER');
  });

  test('refuses a placeholder with no value', () => {
    expect(() => renderIamDocument(template, {})).toThrow(/no value supplied/);
  });

  test('refuses a value no template uses', () => {
    expect(() => renderAll({ ...values, EXTRA: 'x' })).toThrow(/no template uses/);
  });

  test.each([
    ['ACCOUNT_ID', '12345'],
    ['ACCOUNT_ID', '12345678901a'],
    ['CLOUDFORMATION_EXECUTION_ROLE_NAME', 'has space'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve/openbook'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve@0/openbook@2'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve@01/openbook@2'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve@+1/openbook@2'],
    ['GITHUB_REPOSITORY_IDENTITY', 'other@1/openbook@2'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve@1/ens-scout@2'],
    ['GITHUB_REPOSITORY_IDENTITY', 'nbdSteve@1/openbook@2/extra'],
  ])('refuses a malformed %s without echoing it', (placeholder, value) => {
    let message = '';
    try {
      renderAll({ ...values, [placeholder]: value });
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).toMatch(/not shaped like one/);
    expect(message).not.toContain(value);
  });

  test('never prints a supplied value in a refusal', () => {
    let message = '';
    try {
      renderAll({ ...values, GITHUB_REPOSITORY_IDENTITY: secret, ACCOUNT_ID: 'bad' });
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).not.toContain(secret);
  });
});

describe('malformed placeholders', () => {
  const template = '{"Resource": "arn:aws:iam::%PLACEHOLDER%:role/x"}';

  test.each(['${ACCOUNT-ID}', '$ACCOUNT_ID', '${ACCOUNT_ID }', '${ ACCOUNT_ID}', '$${ACCOUNT_ID}', '${}', '${ACCOUNT_ID'])(
    'refuses %s without echoing a value',
    (bad) => {
      let message = '';
      try {
        renderIamDocument(template.replace('%PLACEHOLDER%', () => bad), { ACCOUNT_ID: '123456789012' });
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message).toMatch(/malformed|no value|unknown/);
      expect(message).not.toContain('123456789012');
    },
  );

  test('accepts a well formed placeholder', () => {
    expect(renderIamDocument(template.replace('%PLACEHOLDER%', '${ACCOUNT_ID}'), { ACCOUNT_ID: '123456789012' })).toContain(
      '123456789012',
    );
  });
});

describe('the command line', () => {
  test('refuses an unknown argument, a missing value, and a duplicate', () => {
    expect(() => parseArgv(['--nope'])).toThrow('unknown argument');
    expect(() => parseArgv(['--account-id'])).toThrow(/takes a value/);
    expect(() => parseArgv(['--account-id', '1', '--account-id', '2'])).toThrow(/only once/);
  });

  test('refuses --example with a value of its own', () => {
    expect(() => parseArgv(['--example', '--account-id', '000000000000'])).toThrow(/takes no value/);
  });

  test('refuses to write over the templates', () => {
    expect(() => parseArgv(['--example', '--out-dir', iamDir])).toThrow(/template directory/);
  });

  test('writes the rendered documents to a chosen directory that git ignores', () => {
    const out = path.join(defaultOutDir, `.test-${process.pid}`);
    try {
      const script = path.join(__dirname, '..', 'scripts', 'render-iam.ts');
      const stdout = execFileSync(
        path.join(__dirname, '..', 'node_modules', '.bin', 'tsx'),
        [script, '--example', '--out-dir', out],
        { encoding: 'utf8' },
      );
      expect(stdout).toContain('Apply them out of band');
      expect(fs.readdirSync(out).sort()).toEqual(templateNames().map((n) => `${n}.json`).sort());
    } finally {
      fs.rmSync(out, { recursive: true, force: true });
    }
  });

  test('refuses an output folder that git does not ignore, or that is outside a repository', () => {
    expect(() => assertIgnoredOutDir(iamDir)).toThrow(/git ignores/);
    expect(() => assertIgnoredOutDir(path.join(iamDir, '..', 'lib'))).toThrow(/git ignores/);
    expect(() => assertIgnoredOutDir(path.join(os.tmpdir(), 'openbook-not-a-repo'))).toThrow(/git ignores/);
    expect(() => assertIgnoredOutDir(defaultOutDir)).not.toThrow();
    expect(() => assertIgnoredOutDir(path.join(defaultOutDir, 'a', 'b'))).not.toThrow();
  });

  test('the default output directory is ignored by git', () => {
    const root = path.resolve(__dirname, '..', '..');
    const ignore = fs.readFileSync(path.join(root, '.gitignore'), 'utf8');
    expect(ignore).toContain(`/${path.relative(root, defaultOutDir)}/`);
  });
});
