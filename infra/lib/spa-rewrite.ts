/**
 * CloudFront Functions (cloudfront-js-2.0). The test in `test/spa-rewrite.test.ts` runs this source.
 *
 * - The host redirect: a request for an old host (`ask.steve.gg`, `portal.steve.gg`) gets a 301 to
 *   the area of the new site, with the query string and without the old path. It runs on every
 *   behavior, in front of the cache and the origin.
 * - The SPA rewrite (default behavior only): a path whose last segment has no file extension is a
 *   single-page-app route, so it is served from `/index.html`. That covers `/`, `/ask/...`, and
 *   `/portal/...`. Paths with an extension (assets, config.json) pass through unchanged.
 *
 * A behavior has one viewer-request function, so the default behavior gets both in one function.
 */
export interface HostRedirect {
  /** The old host name, in lower case. */
  host: string;
  /** Where it goes: an absolute https URL. Every path of the old host maps to it. */
  target: string;
}

const redirectPart = (redirects: HostRedirect[]): string => {
  const map = Object.fromEntries(redirects.map((r) => [r.host, r.target]));
  return `var REDIRECTS = ${JSON.stringify(map)};

function queryString(querystring) {
  var parts = [];
  for (var key in querystring) {
    var item = querystring[key];
    if (item.multiValue) {
      for (var i = 0; i < item.multiValue.length; i++) parts.push(key + '=' + item.multiValue[i].value);
    } else {
      parts.push(key + '=' + item.value);
    }
  }
  return parts.join('&');
}

function redirectOf(request) {
  var header = request.headers.host;
  var host = header ? header.value.toLowerCase() : '';
  if (!Object.prototype.hasOwnProperty.call(REDIRECTS, host)) return null;
  var qs = queryString(request.querystring);
  return {
    statusCode: 301,
    statusDescription: 'Moved Permanently',
    headers: { location: { value: qs ? REDIRECTS[host] + '?' + qs : REDIRECTS[host] } }
  };
}
`;
};

/** The source of a viewer-request function: the host redirect, and optionally the SPA rewrite. */
export function viewerRequestCode(redirects: HostRedirect[], spaRewrite: boolean): string {
  const redirect = redirects.length > 0;
  return `${redirect ? redirectPart(redirects) : ''}
function handler(event) {
  var request = event.request;
${redirect ? '  var moved = redirectOf(request);\n  if (moved) return moved;\n' : ''}${
    spaRewrite
      ? `  var uri = request.uri;
  var lastSegment = uri.substring(uri.lastIndexOf('/') + 1);
  if (lastSegment.indexOf('.') === -1) {
    request.uri = '/index.html';
  }
`
      : ''
  }  return request;
}
`.replace(/^\n/, '');
}

/** The default behavior without redirects (dev). */
export const spaRewriteCode = viewerRequestCode([], true);

/**
 * The viewer-response function of the demo behavior. The demo function has a small reserved
 * concurrency, so a busy moment makes Lambda answer 429 itself (a throttle), and a stuck origin gives
 * 5xx. Those answers are not ours: the demo route sets the header `x-openbook-demo` on every
 * response (`crates/api/src/lib.rs`). An answer without it becomes the clean `demo_busy` error.
 */
export const demoResponseCode = `function handler(event) {
  var response = event.response;
  var ours = response.headers['x-openbook-demo'];
  var code = response.statusCode;
  if (!ours && (code === 429 || code >= 500)) {
    return {
      statusCode: 503,
      statusDescription: 'Service Unavailable',
      headers: {
        'content-type': { value: 'application/json' },
        'cache-control': { value: 'no-store' },
        'x-openbook-demo': { value: 'busy' }
      },
      body: { encoding: 'text', data: '{"error":{"code":"demo_busy","message":"The demo is busy. Try again later."}}' }
    };
  }
  return response;
}
`;
