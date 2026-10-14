import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';

type SearchReturn = { to: string; scrollTop: number };

export function searchReturnFrom(state: unknown): SearchReturn | undefined {
  const value = (state as { searchReturn?: SearchReturn } | null)?.searchReturn;
  if (value && typeof value.to === 'string' && /^\/(?:search(?:\?|$)|senders\/-?\d+\?)/.test(value.to) &&
    Number.isFinite(value.scrollTop) && value.scrollTop >= 0) return value;
}

/** Restore only once the cached results (or their first fetch) are rendered. */
export function useRestoreSearchScroll(ready: boolean) {
  const location = useLocation();
  const restored = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (!ready || restored.current === location.key) return;
    restored.current = location.key;
    const top: unknown = location.state?.searchScrollTop;
    const main = document.querySelector<HTMLElement>('.main');
    if (main && typeof top === 'number' && Number.isFinite(top) && top >= 0) main.scrollTop = top;
  }, [ready, location.key, location.state]);
}

export default function SearchResultLink({ to, className, children }: { to: string; className: string; children: ReactNode }) {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <Link to={to} className={className} onClick={(event) => {
      // Keep native open-in-new-tab/window behaviour for modified clicks.
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      const scrollTop = document.querySelector('.main')?.scrollTop ?? 0;
      // Store on both entries: browser Back and the explicit return link restore the same position.
      void navigate(location.pathname + location.search, { replace: true, state: { ...location.state, searchScrollTop: scrollTop } });
      void navigate(to, { state: { searchReturn: { to: location.pathname + location.search, scrollTop } } });
    }}>
      {children}
    </Link>
  );
}
