import type { KeyboardEvent, MouseEvent } from 'react';
import { Link, useNavigate } from 'react-router-dom';

/** Link to a sender page. `inline` renders a span (usable inside another link). */
export default function SenderLink({
  id,
  children,
  inline,
}: {
  id: number | null | undefined;
  children: React.ReactNode;
  inline?: boolean;
}) {
  const navigate = useNavigate();
  if (id == null) return <>{children}</>;
  const to = `/senders/${id}`;
  if (!inline) {
    return (
      <Link className="sender-link" to={to}>
        {children}
      </Link>
    );
  }
  const go = (e: MouseEvent | KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    void navigate(to);
  };
  return (
    <span
      className="sender-link"
      role="link"
      tabIndex={0}
      onClick={go}
      onKeyDown={(e) => e.key === 'Enter' && go(e)}
    >
      {children}
    </span>
  );
}
