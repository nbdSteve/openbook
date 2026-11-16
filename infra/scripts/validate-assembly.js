#!/usr/bin/env node
'use strict';

/**
 * Validates a cloud assembly (cdk.out) before the deploy job lets the CDK CLI read it.
 *
 * The build job produced the assembly, and the build job runs third-party code. The hash that
 * the deploy job checks protects transport only. The CDK CLI would run `source.executable`
 * of an asset with the full environment, after the OIDC credentials step, so this script
 * accepts only the exact shape that the Openbook-prod stack has today and refuses anything
 * else. It is plain Node with no dependency, and the deploy job runs it from the checked-out
 * commit, not from the artifact.
 *
 *   node scripts/validate-assembly.js <cdk.out directory>
 *
 * Exit 0: the assembly has the expected shape. Exit 1: it does not, and the message says why.
 * test/assembly.test.ts runs this script against the real synthesized assembly and against
 * tampered copies.
 */

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const STACK = 'Openbook-prod';
const REGION = 'ap-southeast-2';
const BUCKET = 'cdk-hnb659fds-assets-${AWS::AccountId}-ap-southeast-2';
const PREFIX = 'openbook-prod/';

const ARTIFACTS = {
  [STACK]: 'aws:cloudformation:stack',
  [`${STACK}.assets`]: 'cdk:asset-manifest',
  Tree: 'cdk:tree',
  'aws-cdk-lib/feature-flag-report': 'cdk:feature-flag-report',
};

class Refusal extends Error {}

function refuse(message) {
  throw new Refusal(message);
}

function only(object, allowed, where) {
  if (object === null || typeof object !== 'object' || Array.isArray(object)) refuse(`${where} is not an object`);
  for (const key of Object.keys(object)) {
    if (!allowed.includes(key)) refuse(`${where} has the field "${key}", which is not allowed`);
  }
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return refuse(`${path.basename(file)} is missing or is not JSON`);
  }
}

/** A relative path with no `..`, no absolute part, and no backslash, inside the assembly. */
function insideAssembly(root, relative, where) {
  if (typeof relative !== 'string' || relative === '') refuse(`${where} has no path`);
  if (path.isAbsolute(relative) || relative.includes('\\') || relative.split('/').includes('..')) {
    refuse(`${where} has the path "${relative}", which leaves the assembly`);
  }
  const resolved = path.resolve(root, relative);
  if (resolved !== root && !resolved.startsWith(root + path.sep)) {
    refuse(`${where} has the path "${relative}", which leaves the assembly`);
  }
  return resolved;
}

/** No symlink anywhere: the CLI would follow it when it zips a directory. */
function assertNoSymlinks(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (fs.lstatSync(full).isSymbolicLink()) refuse(`${path.relative(directory, full)} is a symbolic link`);
    if (entry.isDirectory()) assertNoSymlinks(full);
  }
}

function validate(rootArgument) {
  const root = path.resolve(rootArgument);
  if (!fs.existsSync(path.join(root, 'manifest.json'))) refuse(`${rootArgument} has no manifest.json`);
  assertNoSymlinks(root);

  // ---- manifest.json ---------------------------------------------------------------------
  const manifest = readJson(path.join(root, 'manifest.json'));
  only(manifest, ['version', 'artifacts', 'minimumCliVersion', 'runtime'], 'manifest.json');
  only(manifest.artifacts, Object.keys(ARTIFACTS), 'the artifact list');
  for (const [name, type] of Object.entries(ARTIFACTS)) {
    const artifact = manifest.artifacts[name];
    if (!artifact) refuse(`the artifact "${name}" is missing`);
    if (artifact.type !== type) refuse(`the artifact "${name}" has the type "${artifact.type}", expected "${type}"`);
  }

  const stack = manifest.artifacts[STACK];
  // No assumeRoleArn, cloudFormationExecutionRoleArn, lookupRole, or bootstrap fields: the
  // deploy uses the credentials of the job and the role from the command line, and nothing else.
  only(
    stack,
    ['type', 'environment', 'properties', 'dependencies', 'additionalMetadataFile', 'displayName'],
    `the artifact "${STACK}"`,
  );
  if (stack.environment !== `aws://unknown-account/${REGION}`) {
    refuse(`the stack environment is "${stack.environment}", expected an account-agnostic one in ${REGION}`);
  }
  only(
    stack.properties,
    ['templateFile', 'terminationProtection', 'validateOnSynth', 'stackTemplateAssetObjectUrl', 'additionalDependencies'],
    'the stack properties',
  );
  if (stack.properties.templateFile !== `${STACK}.template.json`) refuse('the stack template file is not the expected one');
  const templateUrl = new RegExp(`^s3://${BUCKET.replace(/[$.{}]/g, '\\$&')}/${PREFIX}([0-9a-f]{64})\\.json$`);
  const templateMatch = templateUrl.exec(stack.properties.stackTemplateAssetObjectUrl ?? '');
  if (!templateMatch) refuse('the stack template asset URL is not in the expected bucket and prefix');

  const templatePath = insideAssembly(root, stack.properties.templateFile, 'the stack template');
  const templateHash = crypto.createHash('sha256').update(fs.readFileSync(templatePath)).digest('hex');
  if (templateHash !== templateMatch[1]) refuse('the stack template asset URL is not the hash of the template file');

  // ---- the asset manifest ----------------------------------------------------------------
  const assetsFile = manifest.artifacts[`${STACK}.assets`];
  only(assetsFile, ['type', 'properties'], 'the asset manifest artifact');
  only(assetsFile.properties, ['file'], 'the asset manifest properties');
  if (assetsFile.properties.file !== `${STACK}.assets.json`) refuse('the asset manifest file is not the expected one');

  const assets = readJson(path.join(root, assetsFile.properties.file));
  only(assets, ['version', 'files', 'dockerImages'], `${STACK}.assets.json`);
  if (assets.dockerImages !== undefined && Object.keys(assets.dockerImages).length > 0) {
    refuse('the asset manifest has docker images');
  }
  if (assets.files === null || typeof assets.files !== 'object' || Object.keys(assets.files).length === 0) {
    refuse('the asset manifest has no files');
  }

  let sawTemplate = false;
  for (const [hash, file] of Object.entries(assets.files)) {
    const where = `the asset ${hash.slice(0, 12)}`;
    if (!/^[0-9a-f]{64}$/.test(hash)) refuse(`${where} has a key that is not a SHA-256`);
    only(file, ['displayName', 'source', 'destinations'], where);
    // `executable` (a command that the CLI would run), `packaging` other than zip and file,
    // and every other source field are refused here.
    only(file.source, ['path', 'packaging'], `${where} source`);
    if (file.source.packaging !== 'zip' && file.source.packaging !== 'file') {
      refuse(`${where} has the packaging "${file.source.packaging}"`);
    }
    const source = insideAssembly(root, file.source.path, where);
    if (!fs.existsSync(source)) refuse(`${where} points at a path that does not exist`);
    const isDirectory = fs.statSync(source).isDirectory();
    if (file.source.packaging === 'zip' && !isDirectory) refuse(`${where} is a zip of something that is not a directory`);
    if (file.source.packaging === 'file' && isDirectory) refuse(`${where} is a file asset that is a directory`);

    if (file.destinations === null || typeof file.destinations !== 'object' || Object.keys(file.destinations).length === 0) {
      refuse(`${where} has no destination`);
    }
    for (const [name, destination] of Object.entries(file.destinations)) {
      const dwhere = `${where} destination ${name}`;
      // No assumeRoleArn, assumeRoleExternalId, or assumeRoleAdditionalOptions.
      only(destination, ['bucketName', 'objectKey', 'region'], dwhere);
      if (destination.bucketName !== BUCKET) refuse(`${dwhere} is not in the expected bootstrap bucket`);
      if (destination.region !== REGION) refuse(`${dwhere} is not in ${REGION}`);
      if (!new RegExp(`^${PREFIX}[0-9a-f]{64}\\.(zip|json)$`).test(destination.objectKey ?? '')) {
        refuse(`${dwhere} is not under the ${PREFIX} prefix with a hashed name`);
      }
    }

    // The template asset: its source is the template file, and its hash is the file hash.
    if (file.source.path === stack.properties.templateFile) {
      sawTemplate = true;
      if (hash !== templateHash) refuse('the template asset is not keyed by the hash of the template file');
      if (file.source.packaging !== 'file') refuse('the template asset is not a file asset');
    }
  }
  if (!sawTemplate) refuse('the asset manifest has no template asset');
}

function main(argv) {
  if (argv.length !== 1) {
    process.stderr.write('usage: node scripts/validate-assembly.js <cdk.out directory>\n');
    return 2;
  }
  try {
    validate(argv[0]);
  } catch (error) {
    if (error instanceof Refusal) {
      process.stderr.write(`cloud assembly refused: ${error.message}\n`);
      return 1;
    }
    throw error;
  }
  process.stdout.write('cloud assembly has the expected shape\n');
  return 0;
}

if (require.main === module) {
  process.exitCode = main(process.argv.slice(2));
}

module.exports = { validate, Refusal };
