/**
 * CloudFront Function (viewer request, cloudfront-js-2.0) for the default behavior.
 *
 * A path whose last segment has no file extension is a single-page-app route, so it
 * is served from `/index.html`. Paths with an extension (assets, config.json) pass
 * through unchanged. The test in `test/spa-rewrite.test.ts` runs this exact source.
 */
export const spaRewriteCode = `function handler(event) {
  var request = event.request;
  var uri = request.uri;
  var lastSegment = uri.substring(uri.lastIndexOf('/') + 1);
  if (lastSegment.indexOf('.') === -1) {
    request.uri = '/index.html';
  }
  return request;
}
`;
