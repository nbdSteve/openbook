/**
 * Renders the deployment role documents in `iam/` from their templates.
 *
 * The applied documents name the target account, the CloudFormation execution role, and
 * GitHub's immutable repository identity. None of that belongs in a public repository, so
 * the repository holds templates with an explicit `${PLACEHOLDER}` at each such position.
 *
 * Properties (asserted in `test/iam-render.test.ts`):
 *  1. Deterministic: the same inputs give the same bytes, so a rendered document can be
 *     diffed against the applied one.
 *  2. Fail closed: an unknown placeholder, a missing value, an unused value, or a value
 *     with the wrong shape is a refusal, never a document with a hole in it.
 *  3. No message contains a supplied value.
 */
import * as fs from 'fs';
import * as path from 'path';

export const iamDir = path.join(__dirname, '..', 'iam');
export const templateSuffix = '.template.json';
export const defaultOutDir = path.join(iamDir, 'rendered');

/** The placeholders this script knows, and the shape each value must have. */
export const placeholderRules: Record<string, RegExp> = {
  ACCOUNT_ID: /^[0-9]{12}$/,
  // The characters IAM allows in a role name, and its length limit.
  CLOUDFORMATION_EXECUTION_ROLE_NAME: /^[\w+=,.@-]{1,64}$/,
  // GitHub's immutable subject form: both identities are canonical positive integers.
  GITHUB_REPOSITORY_IDENTITY: /^nbdSteve@[1-9][0-9]*\/openbook@[1-9][0-9]*$/,
};

const placeholderPattern = /\$\{([A-Za-z0-9_]+)\}/g;

export type Values = Record<string, string>;

const has = (obj: object, key: string): boolean => Object.prototype.hasOwnProperty.call(obj, key);

export function templateNames(): string[] {
  return fs
    .readdirSync(iamDir)
    .filter((entry) => entry.endsWith(templateSuffix))
    .map((entry) => entry.slice(0, -templateSuffix.length))
    .sort();
}

export function readTemplate(name: string): string {
  return fs.readFileSync(path.join(iamDir, `${name}${templateSuffix}`), 'utf8');
}

export function placeholdersIn(text: string): Set<string> {
  return new Set([...text.matchAll(placeholderPattern)].map((m) => m[1]));
}

export function exampleValues(): Values {
  return JSON.parse(fs.readFileSync(path.join(iamDir, 'example-values.json'), 'utf8')) as Values;
}

/** Substitutes `values` into one template and returns canonical JSON. */
export function renderIamDocument(text: string, values: Values): string {
  for (const placeholder of placeholdersIn(text)) {
    if (!has(placeholderRules, placeholder)) {
      throw new Error(`unknown placeholder \${${placeholder}}`);
    }
    const value = values[placeholder];
    if (typeof value !== 'string' || value === '') {
      throw new Error(`no value supplied for \${${placeholder}}`);
    }
    if (!placeholderRules[placeholder].test(value)) {
      throw new Error(`the value supplied for \${${placeholder}} is not shaped like one`);
    }
  }
  const substituted = text.replace(placeholderPattern, (match, placeholder: string) =>
    has(values, placeholder) ? values[placeholder] : match,
  );
  return `${JSON.stringify(JSON.parse(substituted), null, 2)}\n`;
}

/** Renders every template. A value that no template uses is refused. */
export function renderAll(values: Values): Map<string, string> {
  const names = templateNames();
  if (names.length === 0) throw new Error(`no ${templateSuffix} document in ${iamDir}`);
  const used = new Set<string>();
  for (const name of names) {
    for (const p of placeholdersIn(readTemplate(name))) used.add(p);
  }
  for (const placeholder of Object.keys(values)) {
    if (!used.has(placeholder)) {
      throw new Error(`a value was supplied for \${${placeholder}}, which no template uses`);
    }
  }
  const rendered = new Map<string, string>();
  for (const name of names) rendered.set(name, renderIamDocument(readTemplate(name), values));
  return rendered;
}

const usage = [
  'usage:',
  '  npm run render:iam -- --example [--out-dir <dir>]',
  '  npm run render:iam -- --account-id <twelve digits>',
  '      --repository-identity <owner@owner-id/repository@repository-id>',
  '      --execution-role-name <name> [--out-dir <dir>]',
].join('\n');

export interface Options {
  example: boolean;
  outDir: string;
  values: Values;
}

export function parseArgv(argv: string[]): Options {
  const options: Options = { example: false, outDir: defaultOutDir, values: {} };
  const flags = new Map([
    ['--account-id', 'ACCOUNT_ID'],
    ['--execution-role-name', 'CLOUDFORMATION_EXECUTION_ROLE_NAME'],
    ['--repository-identity', 'GITHUB_REPOSITORY_IDENTITY'],
  ]);
  for (let i = 0; i < argv.length; i += 1) {
    const argument = argv[i];
    if (argument === '--example') {
      options.example = true;
      continue;
    }
    if (argument === '--out-dir') {
      const next = argv[i + 1];
      if (next === undefined) throw new Error('--out-dir takes a value');
      i += 1;
      options.outDir = path.resolve(next);
      continue;
    }
    const placeholder = flags.get(argument);
    if (placeholder === undefined) throw new Error('unknown argument');
    const next = argv[i + 1];
    if (next === undefined) throw new Error(`${argument} takes a value`);
    if (has(options.values, placeholder)) throw new Error(`${argument} may be supplied only once`);
    i += 1;
    options.values[placeholder] = next;
  }
  if (options.example && Object.keys(options.values).length > 0) {
    throw new Error('--example renders with example-values.json and takes no value of its own');
  }
  // Writing over the templates would put the rendered account into the repository.
  if (options.outDir === iamDir) throw new Error('--out-dir must not be the template directory itself');
  return options;
}

function main(argv: string[]): void {
  const options = parseArgv(argv);
  const rendered = renderAll(options.example ? exampleValues() : options.values);
  fs.mkdirSync(options.outDir, { recursive: true });
  for (const [name, document] of rendered) {
    const target = path.join(options.outDir, `${name}.json`);
    fs.writeFileSync(target, document);
    process.stdout.write(`${target}\n`);
  }
  process.stdout.write('These are not committed. Apply them out of band.\n');
}

if (require.main === module) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.stderr.write(`${usage}\n`);
    process.exitCode = 1;
  }
}
