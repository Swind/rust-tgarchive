import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';

export default function SenderLink({ id, children }: { id: number | null | undefined; children: ReactNode }) {
  if (id == null) return <>{children}</>;
  return <Link className="sender-link rounded-sm underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring" to={`/senders/${id}`}>{children}</Link>;
}
