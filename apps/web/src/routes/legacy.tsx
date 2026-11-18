import { Navigate, useLocation, useParams, type Params } from 'react-router';

export function LegacyRedirect({ to }: { to: (params: Params) => string }) {
  const params = useParams();
  const { search, hash } = useLocation();
  return <Navigate to={`${to(params)}${search}${hash}`} replace />;
}
