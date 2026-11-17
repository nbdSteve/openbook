import { Navigate, useLocation, useParams, type Params } from 'react-router';

/** An old address that still works: it sends the visitor to the new one and keeps the query and the hash. */
export function LegacyRedirect({ to }: { to: (params: Params) => string }) {
  const params = useParams();
  const { search, hash } = useLocation();
  return <Navigate to={`${to(params)}${search}${hash}`} replace />;
}
