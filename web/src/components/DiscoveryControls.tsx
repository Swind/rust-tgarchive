import { useId, type ReactNode } from 'react';
import { ChevronDown, Search, SlidersHorizontal } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';

export function SecondaryFilters({ count, children }: { count: number; children: ReactNode }) {
  return <Collapsible className="group/filters">
    <CollapsibleTrigger asChild>
      <Button type="button" variant="outline" className="w-full justify-start md:hidden">
        <SlidersHorizontal aria-hidden="true" />篩選條件
        {count > 0 && <span className="rounded-full bg-primary/10 px-2 py-0.5 text-xs text-primary">已套用 {count} 項</span>}
        <ChevronDown aria-hidden="true" className="ms-auto group-data-[state=open]/filters:rotate-180" />
      </Button>
    </CollapsibleTrigger>
    <CollapsibleContent forceMount className="mt-4 space-y-4 data-[state=closed]:hidden md:mt-0 md:data-[state=closed]:block">
      {children}
    </CollapsibleContent>
  </Collapsible>;
}

export const SEARCH_SORTS = [['relevance', '相關性'], ['time', '時間']] as const;

export function SearchField({ label, placeholder, value, onChange, autoFocus }: {
  label: string; placeholder: string; value: string; onChange: (value: string) => void; autoFocus?: boolean;
}) {
  const id = useId();
  return <div className="grid min-w-0 gap-2">
    <Label htmlFor={id}>{label}</Label>
    <div className="relative">
      <Search aria-hidden="true" className="pointer-events-none absolute top-1/2 left-3.5 size-4.5 -translate-y-1/2 text-muted-foreground" />
      <Input id={id} type="search" className="h-12 bg-background ps-11 text-base md:text-base" autoFocus={autoFocus}
        autoCapitalize="none" autoCorrect="off" spellCheck={false} enterKeyHint="search" placeholder={placeholder}
        value={value} onChange={(e) => onChange(e.target.value)} />
    </div>
  </div>;
}

export function Choices<T extends string>({ label, choices, value, onChange }: {
  label: string; choices: ReadonlyArray<readonly [T, string]>; value: T; onChange: (value: T) => void;
}) {
  return <div role="group" aria-label={label} className="flex flex-wrap items-center gap-x-3 gap-y-1">
    <span className="text-xs font-medium text-muted-foreground">{label}</span>
    <div className="inline-flex flex-wrap gap-0.5 rounded-lg bg-muted p-1">
      {choices.map(([key, name]) => <Button key={key} size="sm" type="button" variant="ghost" aria-pressed={key === value}
        className="h-8 px-2.5 text-xs text-muted-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm pointer-coarse:min-h-11"
        onClick={() => onChange(key)}>{name}</Button>)}
    </div>
  </div>;
}

export function ChatFilter({ value, onChange, children }: { value: string; onChange: (value: string) => void; children: ReactNode }) {
  const id = useId();
  return <div className="grid min-w-0 gap-2 [&>[data-slot=native-select-wrapper]]:w-full">
    <Label htmlFor={id}>聊天室</Label>
    <NativeSelect id={id} className="h-10 truncate bg-background text-base md:text-sm" value={value} onChange={(e) => onChange(e.target.value)}>
      <NativeSelectOption value="">所有聊天室</NativeSelectOption>
      {children}
    </NativeSelect>
  </div>;
}

export function DateRange({ from, to, update }: { from: string; to: string; update: (changes: Record<string, string>) => void }) {
  const id = useId();
  return <fieldset className="date-range min-w-0">
    <legend className="mb-2 text-sm leading-none font-medium">日期範圍</legend>
    <div className="grid gap-2 @sm:grid-cols-2">
      {([['from', '從', from], ['to', '到', to]] as const).map(([key, label, date]) => (
        <Label key={key} htmlFor={`${id}-${key}`} className="flex min-w-0 gap-2 text-xs text-muted-foreground">
          {label}<Input id={`${id}-${key}`} type="date" className="h-10 min-w-0 bg-background text-base text-foreground md:text-sm"
            value={date} onChange={(e) => update({ [key]: e.target.value })} />
        </Label>
      ))}
    </div>
  </fieldset>;
}

export function CheckFilter({ label, checked, onChange }: { label: string; checked: boolean; onChange: (checked: boolean) => void }) {
  const id = useId();
  return <Label htmlFor={id} className="min-h-10 cursor-pointer gap-2 font-normal pointer-coarse:min-h-12">
    <Checkbox id={id} checked={checked} onCheckedChange={(next) => onChange(next === true)} />{label}
  </Label>;
}
