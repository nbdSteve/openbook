/**
 * The CloudFront Function of the default behavior (viewer request, cloudfront-js-2.0). The test in
 * `test/edge.test.ts` runs this exact source.
 *
 * - The host redirect: a request for an old host gets a redirect to the new site.
 *   - `keepPath` hosts (`ask.steve.gg`) keep the path and the query string:
 *     `ask.steve.gg/join/CODE?a=1` goes to `openbook.steve.gg/join/CODE?a=1`. The new web app
 *     redirects the old app routes (`/app/...`, `/login`) on the client and keeps `/join/:code`, so an
 *     invite link keeps its code.
 *   - The other hosts (`portal.steve.gg`) go to one fixed address (`/portal`), because the old routes
 *     do not exist in the new app.
 *   - `/api` and `/api/*` are never redirected: the API works on any host, and a redirected `POST`
 *     becomes a cross-origin `GET`. (They are not served by this behavior anyway.)
 *   - The status is 302 for now. Switch to 301 after the release is verified, because browsers keep a
 *     301 with no end date.
 * - The SPA rewrite: a path whose last segment has no file extension is a single-page-app route, so it
 *   is served from `/index.html`. That covers `/`, `/ask/...`, and `/portal/...`. Paths with an
 *   extension (assets, config.json) pass through unchanged.
 *
 * A behavior has one viewer-request function, so the default behavior gets both in one function.
 */
export interface HostRedirect {
  /** The old host name, in lower case. */
  host: string;
  /** An absolute https URL without a trailing slash: the site, or the fixed address. */
  target: string;
  /** True: the target is the site and the old path (and query) is kept. False: the target is the whole address. */
  keepPath: boolean;
}

/** The status of a host redirect. 302 until the release is verified, then 301. */
export const redirectStatus = 302;

const redirectPart = (redirects: HostRedirect[]): string => {
  const map = Object.fromEntries(redirects.map((r) => [r.host, { target: r.target, keepPath: r.keepPath }]));
  return `var REDIRECTS = ${JSON.stringify(map)};

function normalizeHost(value) {
  var host = value.toLowerCase();
  if (host.slice(-4) === ':443') host = host.slice(0, -4);
  if (host.charAt(host.length - 1) === '.') host = host.slice(0, -1);
  return host;
}

function part(key, value) {
  return value === '' ? key : key + '=' + value;
}

function queryString(querystring) {
  var parts = [];
  for (var key in querystring) {
    var item = querystring[key];
    if (item.multiValue) {
      for (var i = 0; i < item.multiValue.length; i++) parts.push(part(key, item.multiValue[i].value));
    } else {
      parts.push(part(key, item.value));
    }
  }
  return parts.join('&');
}

function redirectOf(request) {
  var uri = request.uri;
  if (uri === '/api' || uri.indexOf('/api/') === 0) return null;
  var header = request.headers.host;
  var host = header ? normalizeHost(header.value) : '';
  if (!Object.prototype.hasOwnProperty.call(REDIRECTS, host)) return null;
  var rule = REDIRECTS[host];
  var location = rule.keepPath ? rule.target + uri : rule.target;
  var qs = queryString(request.querystring);
  return {
    statusCode: ${redirectStatus},
    statusDescription: 'Found',
    headers: { location: { value: qs ? location + '?' + qs : location } }
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
