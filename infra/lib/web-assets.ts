import * as fs from 'fs';
import * as path from 'path';

/**
 * The CSP is `script-src 'self'`, so the browser refuses an inline script. A build with one
 * would deploy a blank page, so synth refuses it instead.
 *
 * Data blocks (`type="application/json"` and `application/ld+json`) do not run, so they
 * are allowed.
 */
export function assertNoInlineScripts(webAssetDir: string): void {
  const indexPath = path.join(webAssetDir, 'index.html');
  if (!fs.existsSync(indexPath)) {
    throw new Error(`${indexPath} does not exist. Build the web app first (make build-web).`);
  }
  const html = fs.readFileSync(indexPath, 'utf8');
  for (const match of html.matchAll(/<script\b([^>]*)>/gi)) {
    const attributes = match[1];
    if (/\bsrc\s*=/i.test(attributes)) continue;
    if (/\btype\s*=\s*["']?application\/(ld\+)?json["']?/i.test(attributes)) continue;
    throw new Error(
      `${indexPath} has an inline <script>. The Content-Security-Policy (script-src 'self') blocks it. ` +
        'Move the code to a file under public/ or src/ and load it with <script src>.',
    );
  }
}
