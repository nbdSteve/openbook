import * as fs from 'fs';
import * as path from 'path';

/**
 * A Lambda asset directory must hold the files that the function needs at start. A missing
 * file would deploy a function that fails at every invocation, so synth refuses it.
 * `openbook-ingest` also needs `libpdfium.so` next to its `bootstrap` (make build-lambdas copies it).
 */
export function assertLambdaAssetDir(dir: string, files: string[], hint: string): void {
  for (const file of files) {
    const full = path.join(dir, file);
    if (!fs.existsSync(full) || !fs.statSync(full).isFile()) {
      throw new Error(`${full} does not exist. ${hint}`);
    }
  }
}
