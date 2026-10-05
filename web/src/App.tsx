import { Navigate, Route, Routes } from 'react-router-dom';
import Layout from './components/Layout';
import ChatsPage from './pages/ChatsPage';
import SearchPage from './pages/SearchPage';
import SendersPage from './pages/SendersPage';
import SenderPage from './pages/SenderPage';
import SyncPage from './pages/SyncPage';
import StatusPage from './pages/StatusPage';

export default function App() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Navigate to="/chats" replace />} />
        <Route path="chats" element={<ChatsPage />} />
        <Route path="chats/:chatId" element={<ChatsPage />} />
        <Route path="search" element={<SearchPage />} />
        <Route path="senders" element={<SendersPage />} />
        <Route path="senders/:senderId" element={<SenderPage />} />
        <Route path="sync" element={<SyncPage />} />
        <Route path="status" element={<StatusPage />} />
        <Route path="*" element={<div className="empty">找不到頁面</div>} />
      </Route>
    </Routes>
  );
}
