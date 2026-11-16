import { existsSync, readFileSync, rmSync } from 'node:fs';
import { DEPLOYED, REGISTRY, deleteUser } from './helpers';

/**
 * Deployed mode: delete every user that the run created, also when tests failed or a worker
 * restarted. Local mode needs no cleanup, because the stack is thrown away.
 */
export default async function globalTeardown(): Promise<void> {
  if (!DEPLOYED || !existsSync(REGISTRY)) return;
  const lines = readFileSync(REGISTRY, 'utf8').split('\n').filter(Boolean);
  let failed = 0;
  for (const line of lines) {
    const { email, userPoolId } = JSON.parse(line) as { email: string; userPoolId: string };
    try {
      await deleteUser(email, userPoolId);
    } catch (error) {
      // A user that never got created (the create call failed) is not an error.
      if ((error as { name?: string }).name === 'UserNotFoundException') continue;
      failed++;
      console.error(`Could not delete ${email}: ${(error as Error).message}`);
    }
  }
  console.log(`Deleted ${lines.length - failed} of ${lines.length} users created by this run.`);
  if (failed === 0) rmSync(REGISTRY);
}
