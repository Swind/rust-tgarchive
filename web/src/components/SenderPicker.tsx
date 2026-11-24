import { useEffect, useId, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList } from '@/components/ui/combobox';
import { Label } from '@/components/ui/label';
import { api, type SenderProfile } from '../api/client';
import { senderLabel } from './senderFormat';
import { ErrorBox, useDebounced } from './common';

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
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const clear = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
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
    if (value !== undefined) {
      setText('');
      setOpen(false);
    }
    if (restoreFocus.current) {
      (value === undefined ? input.current : clear.current)?.focus();
      restoreFocus.current = false;
    }
  }, [value]);

  function select(next: number | undefined) {
    restoreFocus.current = true;
    onChange(next);
  }
  const matches = debounced === text.trim() ? options.data?.items : undefined;

  return (
    <div className="relative grid min-w-0 gap-2">
      {value !== undefined ? <>
        <span className="text-sm leading-none font-medium">寄件人</span>
        <div className="flex h-10 min-w-0 items-center justify-between gap-1 rounded-md border border-primary/30 bg-primary/5 ps-3 text-sm font-medium">
          <span className="truncate">寄件人：{selected.data ? senderLabel(selected.data) : value}</span>
          <Button ref={clear} type="button" variant="ghost" size="icon" aria-label="清除寄件人" onClick={() => select(undefined)}><X aria-hidden="true" /></Button>
        </div>
      </> : <>
        <Label htmlFor={id}>寄件人</Label>
        <Combobox<SenderProfile> items={matches ?? []} filter={null} inputValue={text} onInputValueChange={setText}
          openOnInputClick={false} onOpenChange={setOpen} itemToStringLabel={senderLabel}
          onValueChange={(sender) => { if (sender) select(sender.id); }}>
          <ComboboxInput ref={input} id={id} showTrigger={false} className="h-10 bg-background" autoCapitalize="none"
            autoCorrect="off" spellCheck={false} placeholder="名稱或 @帳號" />
          <ComboboxContent>
            {options.error ? <ErrorBox error={options.error} /> : <ComboboxEmpty>
              {text.trim().length === 0 ? '輸入名稱或 @帳號' : matches ? '沒有符合的使用者' : '搜尋中…'}
            </ComboboxEmpty>}
            <ComboboxList aria-label="寄件人建議">
              {(s: SenderProfile) => <ComboboxItem key={s.id} value={s}>
                <span className="min-w-0 break-all">{senderLabel(s)}{s.username && s.display_name ? ` @${s.username}` : ''}</span>
                <span className="ms-auto shrink-0 text-xs text-muted-foreground">{s.message_count} 則</span>
              </ComboboxItem>}
            </ComboboxList>
          </ComboboxContent>
        </Combobox>
      </>}
    </div>
  );
}
