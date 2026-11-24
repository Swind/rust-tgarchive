import { useId, useMemo, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { Folder, Pencil, Plus, Trash2 } from 'lucide-react';
import { api, type Chat, type SearchGroup } from '@/api/client';
import { ErrorBox } from '@/components/common';
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { chatTitle } from '@/pages/ChatsPage';

interface Props {
  groups: SearchGroup[];
  chats: Chat[];
  selectedId: number | undefined;
  onSelect: (id: number | undefined) => void;
}

type Draft = { original?: SearchGroup; name: string; members: Set<number> };

export function SearchGroupManager({ groups, chats, selectedId, onSelect }: Props) {
  const id = useId();
  const cache = useQueryClient();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [filter, setFilter] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const original = draft?.original;
  const originalIds = original?.chat_ids;
  const dirty = draft !== null && (draft.name !== (original?.name ?? '') ||
    draft.members.size !== (originalIds?.length ?? 0) || originalIds?.some((member) => !draft.members.has(member)));
  const name = draft?.name.trim() ?? '';
  const nameLength = [...name].length;

  function finish(saved: SearchGroup | undefined, changedId: number) {
    cache.setQueryData<SearchGroup[]>(['search-groups'], (current) => {
      if (!current) return current;
      if (!saved) return current.filter((group) => group.id !== changedId);
      return current.some((group) => group.id === changedId)
        ? current.map((group) => group.id === changedId ? saved : group)
        : [...current, saved];
    });
    if (saved || changedId === selectedId) onSelect(saved?.id);
    setConfirmDelete(false);
    setOpen(false);
    void cache.invalidateQueries({ queryKey: ['search-groups'] });
    void cache.invalidateQueries({ queryKey: ['search'] });
  }

  const save = useMutation({
    mutationFn: (editing: Draft) => {
      const body = { name: editing.name.trim(), chat_ids: [...editing.members] };
      return editing.original ? api.updateSearchGroup(editing.original.id, body) : api.createSearchGroup(body);
    },
    onSuccess: (saved) => finish(saved, saved.id),
  });
  const remove = useMutation({
    mutationFn: (groupId: number) => api.deleteSearchGroup(groupId),
    onSuccess: (_, groupId) => finish(undefined, groupId),
  });
  const pending = save.isPending || remove.isPending;

  function changeOpen(next: boolean) {
    if (!next && (pending || (dirty && !window.confirm('捨棄尚未儲存的變更？')))) return;
    if (next) {
      setDraft(null);
      setFilter('');
      save.reset();
      remove.reset();
    }
    setOpen(next);
  }

  const editing = open && draft !== null;
  const options = useMemo(() => {
    if (!editing) return [];
    const titles = new Map(chats.map((chat) => [chat.id, chatTitle(chat)]));
    // A settings group can outlive a chat in the archive. Keep such members visible and removable.
    for (const member of originalIds ?? []) if (!titles.has(member)) titles.set(member, '不在目前封存中');
    const query = filter.trim().toLowerCase();
    return [...titles].filter(([member, title]) => !query || title.toLowerCase().includes(query) || String(member).includes(query));
  }, [editing, chats, originalIds, filter]);

  function edit(group?: SearchGroup) {
    setDraft({ original: group, name: group?.name ?? '', members: new Set(group?.chat_ids) });
  }

  return <Dialog open={open} onOpenChange={changeOpen}>
    <DialogTrigger asChild><Button type="button" variant="outline"><Folder aria-hidden="true" />管理搜尋群組</Button></DialogTrigger>
    <DialogContent showCloseButton={false} className="max-h-[90dvh] overflow-y-auto overscroll-contain p-4 sm:p-6">
        <DialogHeader>
          <DialogTitle>{draft ? original ? '編輯搜尋群組' : '新增搜尋群組' : '管理搜尋群組'}</DialogTitle>
          <DialogDescription>將聊天室或頻道存成群組，搜尋時只查看群組內的訊息。</DialogDescription>
        </DialogHeader>
        {!draft ? <>
          <Button type="button" className="justify-self-start" onClick={() => edit()}><Plus aria-hidden="true" />新增群組</Button>
          {groups.length === 0 ? <p className="text-sm text-muted-foreground">還沒有搜尋群組。</p> :
            <ul className="divide-y rounded-md border">
              {groups.map((group) => <li key={group.id} className="flex items-center justify-between gap-2 px-3 py-2">
                <div className="min-w-0">
                  <p className="break-words text-sm font-medium">{group.name}</p>
                  <p className="text-xs text-muted-foreground">{group.chat_ids.length} 個成員{selectedId === group.id && ' · 目前搜尋範圍'}</p>
                </div>
                <Button type="button" variant="outline" size="sm" aria-label={`編輯 ${group.name}`} onClick={() => edit(group)}><Pencil aria-hidden="true" />編輯</Button>
              </li>)}
            </ul>}
        </> : <>
          <form id={`${id}-form`} className="grid gap-2" onSubmit={(event) => {
            event.preventDefault();
            if (!pending && nameLength > 0 && nameLength <= 80 && dirty) save.mutate(draft);
          }}>
            <Label htmlFor={`${id}-name`}>群組名稱</Label>
            <Input id={`${id}-name`} autoFocus required value={draft.name} disabled={pending} aria-describedby={`${id}-limit`}
              aria-invalid={nameLength > 80} onChange={(event) => setDraft({ ...draft, name: event.target.value })} />
            <p id={`${id}-limit`} className="text-xs text-muted-foreground">名稱限 1–80 字{ nameLength > 80 ? `，目前為 ${nameLength} 字。` : '。' }</p>
          </form>
          <fieldset className="grid min-w-0 gap-2" disabled={pending}>
            <legend className="mb-2 text-sm font-medium">群組成員 · 已選 {draft.members.size} / 500</legend>
            <Label htmlFor={`${id}-filter`}>搜尋聊天室</Label>
            <Input id={`${id}-filter`} type="search" placeholder="輸入聊天室或頻道名稱、ID" value={filter} onChange={(event) => setFilter(event.target.value)} />
            <div className="max-h-64 divide-y overflow-y-auto overscroll-contain rounded-md border">
              {options.map(([member, title]) => <Label key={member} htmlFor={`${id}-chat-${member}`} className="min-h-11 cursor-pointer gap-3 px-3 py-2 font-normal hover:bg-muted/50">
                <Checkbox id={`${id}-chat-${member}`} checked={draft.members.has(member)} disabled={pending || (!draft.members.has(member) && draft.members.size >= 500)}
                  onCheckedChange={(checked) => {
                    const members = new Set(draft.members);
                    if (checked === true) members.add(member); else members.delete(member);
                    setDraft({ ...draft, members });
                  }} />
                <span className="min-w-0 break-words">{title}{title !== String(member) && <span className="block text-xs text-muted-foreground">{member}</span>}</span>
              </Label>)}
              {options.length === 0 && <p className="p-3 text-sm text-muted-foreground">{filter.trim() ? '找不到符合的聊天室或頻道。' : '沒有可加入的聊天室或頻道。'}</p>}
            </div>
            {draft.members.size === 0 && <p className="text-xs text-muted-foreground">空群組不會顯示任何搜尋結果。</p>}
          </fieldset>
          <ErrorBox error={save.error} />
        </>}
        <footer className="flex flex-wrap justify-end gap-2">
          {original && <AlertDialog open={confirmDelete} onOpenChange={(next) => {
            if (!remove.isPending) { remove.reset(); setConfirmDelete(next); }
          }}>
            <AlertDialogTrigger asChild><Button type="button" variant="ghost" className="me-auto text-destructive" disabled={pending}><Trash2 aria-hidden="true" />刪除群組</Button></AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle className="break-all">刪除「{original.name}」？</AlertDialogTitle>
                <AlertDialogDescription>只會刪除搜尋群組，不會刪除任何封存的聊天室、頻道或訊息。此操作無法復原。{original.id === selectedId && '搜尋也會解除此群組限制。'}</AlertDialogDescription>
              </AlertDialogHeader>
              <ErrorBox error={remove.error} />
              <AlertDialogFooter>
                <AlertDialogCancel disabled={remove.isPending}>取消</AlertDialogCancel>
                <AlertDialogAction variant="destructive" disabled={remove.isPending} onClick={(event) => {
                  event.preventDefault();
                  remove.mutate(original.id);
                }}>{remove.isPending ? '刪除中…' : '確認刪除'}</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>}
          <DialogClose asChild><Button type="button" variant="outline" disabled={pending}>{draft ? '取消' : '關閉'}</Button></DialogClose>
          {draft && <Button type="submit" form={`${id}-form`} disabled={pending || !dirty || !nameLength || nameLength > 80}>
            {save.isPending ? '儲存中…' : '儲存群組'}
          </Button>}
        </footer>
    </DialogContent>
  </Dialog>;
}
