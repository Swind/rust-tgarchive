import { NavLink, Outlet } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { api } from '../api/client';
import { currentTheme, setTheme } from '../lib/theme';

const TONE: Record<string, string> = {
  running: 'green',
  catching_up: 'blue',
  starting: 'blue',
  reconnecting: 'amber',
  degraded: 'amber',
  failed: 'red',
  disabled: 'gray',
};

function CollectorBadge() {
  const { data, error } = useQuery({ queryKey: ['status'], queryFn: api.status, refetchInterval: 5000 });
  const state = data?.collector.state;
  const title = error ? (error as Error).message : (data?.collector.detail ?? state ?? '載入中');
  return (
    <span className={`badge ${state ? (TONE[state] ?? 'gray') : error ? 'red' : 'gray'}`} title={title}>
      收集器：{state ?? (error ? '無法取得' : '…')}
    </span>
  );
}

function ThemeToggle() {
  const [theme, setT] = useState(currentTheme);
  const next = theme === 'dark' ? 'light' : 'dark';
  return (
    <button
      className="icon-btn"
      aria-label={`切換為${next === 'dark' ? '深色' : '淺色'}主題`}
      onClick={() => {
        setTheme(next);
        setT(next);
      }}
    >
      {theme === 'dark' ? '☀️' : '🌙'}
    </button>
  );
}

export default function Layout() {
  return (
    <div className="app">
      <header className="topbar">
        <strong className="brand">tgarchive</strong>
        <nav aria-label="主選單">
          <NavLink to="/chats">對話</NavLink>
          <NavLink to="/search">搜尋</NavLink>
          <NavLink to="/senders">使用者</NavLink>
          <NavLink to="/sync">同步</NavLink>
          <NavLink to="/status">狀態</NavLink>
        </nav>
        <span className="spacer" />
        <CollectorBadge />
        <ThemeToggle />
      </header>
      <main className="main">
        <Outlet />
      </main>
    </div>
  );
}
