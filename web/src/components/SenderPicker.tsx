import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import { senderLabel } from './senderFormat';
import { useDebounced } from './common';

/** Sender filter with autocomplete from `/senders?q=`; `value` is the selected sender ID. */
export default function SenderPicker({
  value,
  onChange,
}: {
  value: number | undefined;
  onChange: (id: number | undefined) => void;
}) {
  const [text, setText] = useState('');
  const [open, setOpen] = useState(false);
  const debounced = useDebounced(text.trim(), 250);
  const selected = useQuery({
    queryKey: ['sender', value],
    enabled: value !== undefined,
    queryFn: () => api.sender(value as number),
    retry: false,
  });
  const options = useQuery({
    queryKey: ['senders-pick', debounced],
    enabled: open && debounced.length > 0,
    queryFn: () => api.senderList({ q: debounced, limit: 8 }),
  });
  useEffect(() => {
    if (value === undefined) return;
    setText('');
    setOpen(false);
  }, [value]);

  if (value !== undefined) {
    return (
      <span className="badge blue sender-chip">
        寄件人：{selected.data ? senderLabel(selected.data) : value}{' '}
        <button type="button" aria-label="清除寄件人" onClick={() => onChange(undefined)}>✕</button>
      </span>
    );
  }
  return (
    <span className="combo">
      <input
        type="search"
        role="combobox"
        aria-label="寄件人"
        aria-expanded={open && debounced.length > 0}
        aria-autocomplete="list"
        placeholder="寄件人（名稱或 @帳號）"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={(e) => e.key === 'Escape' && setOpen(false)}
      />
      {open && debounced.length > 0 && options.data && (
        <ul className="combo-list" role="listbox" aria-label="寄件人建議">
          {options.data.items.length === 0 && <li className="muted pad">沒有符合的使用者</li>}
          {options.data.items.map((s) => (
            <li key={s.id} role="option" aria-selected={false}>
              <button type="button" onClick={() => onChange(s.id)}>
                {senderLabel(s)}
                {s.username && s.display_name ? ` @${s.username}` : ''}
                <span className="muted small"> · {s.message_count} 則</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </span>
  );
}
