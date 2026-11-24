import { Link, NavLink, Outlet } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Activity, Archive, MessagesSquare, Moon, RefreshCw, Search, Sun, UsersRound } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { api } from '../api/client';
import { currentTheme, setTheme, storedTheme } from '../lib/theme';

const destinations = [
  { to: '/chats', label: '對話', Icon: MessagesSquare },
  { to: '/search', label: '搜尋', Icon: Search },
  { to: '/senders', label: '使用者', Icon: UsersRound },
  { to: '/sync', label: '同步', Icon: RefreshCw },
  { to: '/status', label: '狀態', Icon: Activity },
];

const collectorStates: Record<string, string> = {
  running: '運作中', catching_up: '追趕進度', starting: '啟動中', reconnecting: '重新連線中',
  degraded: '服務異常', failed: '失敗', disabled: '已停用',
};

function CollectorBadge() {
  const { data, error } = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 5000 });
  const state = data?.collector.state;
  const title = error ? (error as Error).message : (data?.collector.detail ?? state ?? '載入中');
  return (
    <Badge variant="outline" asChild>
      <Link to="/status" title={title} className="min-h-8 gap-2 px-3">
        <span aria-hidden="true" className={`size-2 rounded-full border border-current ${state === 'disabled' && !error ? '' : 'bg-current'}`} />
        收集器：{error ? '無法取得' : state ? (collectorStates[state] ?? state) : '載入中'}
      </Link>
    </Badge>
  );
}

function ThemeToggle() {
  const [theme, setT] = useState(currentTheme);
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    function followSystem() {
      if (storedTheme()) return;
      const theme = currentTheme();
      document.documentElement.dataset.theme = theme;
      setT(theme);
    }
    media.addEventListener('change', followSystem);
    return () => media.removeEventListener('change', followSystem);
  }, []);
  const next = theme === 'dark' ? 'light' : 'dark';
  return (
    <Button size="icon" variant="ghost"
      aria-label={`切換為${next === 'dark' ? '深色' : '淺色'}主題`}
      onClick={() => {
        setTheme(next);
        setT(next);
      }}
    >
      {theme === 'dark' ? <Sun aria-hidden="true" /> : <Moon aria-hidden="true" />}
    </Button>
  );
}

export default function Layout() {
  return (
    <div className="flex h-dvh min-h-0 flex-col pb-[calc(3.75rem+env(safe-area-inset-bottom))] md:pb-0">
      <a href="#archive-content" className="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:bg-card focus:p-3">跳至主要內容</a>
      <header className="flex min-h-14 shrink-0 items-center gap-3 border-b bg-card px-4 md:min-h-16 lg:gap-6 lg:px-6">
        <Link to="/chats" className="flex shrink-0 items-center gap-2 font-semibold tracking-tight">
          <span className="grid size-8 place-items-center rounded-lg bg-primary text-primary-foreground"><Archive className="size-[18px]" aria-hidden="true" /></span>
          <span className="text-base">tgarchive</span>
        </Link>
        <nav aria-label="主選單" className="fixed inset-x-0 bottom-0 z-20 grid grid-cols-5 border-t bg-card px-1 pb-[env(safe-area-inset-bottom)] md:static md:flex md:border-0 md:border-s md:ps-2 md:pb-0 lg:gap-1 lg:ps-5">
          {destinations.map(({ to, label, Icon }) => (
            <Button asChild key={to} variant="ghost" className="h-[3.75rem] min-w-0 flex-col gap-1 rounded-none px-1 text-xs text-muted-foreground aria-[current=page]:bg-accent aria-[current=page]:font-bold aria-[current=page]:text-accent-foreground md:h-9 md:flex-row md:gap-2 md:rounded-md md:px-3 md:text-sm">
              <NavLink to={to}><Icon aria-hidden="true" />{label}</NavLink>
            </Button>
          ))}
        </nav>
        <div className="ms-auto flex items-center gap-1">
          <CollectorBadge />
          <ThemeToggle />
        </div>
      </header>
      <main id="archive-content" tabIndex={-1} className="relative min-h-0 min-w-0 flex-1 overflow-auto focus:outline-none">
        <Outlet />
      </main>
    </div>
  );
}
