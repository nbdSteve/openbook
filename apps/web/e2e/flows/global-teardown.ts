import { existsSync, readFileSync, rmSync } from 'node:fs';
import { DEPLOYED, REGISTRY, deleteUser } from './helpers';

export default async function globalTeardown(): Promise<void> {
  if (!DEPLOYED || !existsSync(REGISTRY)) return;
  const lines = readFileSync(REGISTRY, 'utf8').split('\n').filter(Boolean);
  let failed = 0;
  for (const line of lines) {
    const { email, userPoolId } = JSON.parse(line) as { email: string; userPoolId: string };
    try {
      await deleteUser(email, userPoolId);
    } catch (error) {
      if ((error as { name?: string }).name === 'UserNotFoundException') continue;
      failed++;
      console.error(`Could not delete ${email}: ${(error as Error).message}`);
    }
  }
  console.log(`Deleted ${lines.length - failed} of ${lines.length} users created by this run.`);
  if (failed === 0) rmSync(REGISTRY);
}
