import { Alert } from '@/components/ui/Alert';
import { Spinner } from '@/components/ui/Spinner';
import { formatDateTime } from '@/lib/format';
import { useResource } from '@/lib/useResource';
import { describeApiError } from '@/services/api/errors';
import { getInsights } from '@/services/api/routes';
import { useCourse } from '../CourseContext';

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-2xl border border-border bg-surface p-4 sm:p-5">
      <p className="text-3xl font-bold text-text-primary">{value}</p>
      <p className="mt-1 text-sm text-text-secondary">{label}</p>
    </div>
  );
}

export function InsightsTab() {
  const { course } = useCourse();
  const insights = useResource(`insights:${course.id}`, (signal) => getInsights(course.id, signal));

  if (insights.loading) return <Spinner label="Loading insights" />;
  if (!insights.data) return <Alert tone="error">{describeApiError(insights.error)}</Alert>;
  const { totals, questions } = insights.data;

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-2 gap-3 sm:gap-4">
        <Stat label="Questions in the last 7 days" value={totals.questionsLast7Days} />
        <Stat label="Active students in the last 7 days" value={totals.activeStudentsLast7Days} />
      </div>
      <section aria-labelledby="questions-heading">
        <h3 id="questions-heading" className="mb-3 text-base font-semibold text-text-primary">
          Latest student questions
        </h3>
        {questions.length === 0 ? (
          <div className="rounded-2xl border border-border bg-surface px-4 py-8 text-center text-sm text-text-secondary">
            No student questions yet. Questions from students who join this course show here. Your own questions are not listed.
          </div>
        ) : (
          <ul className="space-y-2">
            {questions.map((q) => (
              <li key={q.id} className="rounded-xl border border-border bg-surface p-4">
                <p className="whitespace-pre-wrap break-words text-sm text-text-primary">{q.question}</p>
                <p className="mt-2 text-xs text-text-muted">
                  {q.email} · {formatDateTime(q.createdAt)}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
