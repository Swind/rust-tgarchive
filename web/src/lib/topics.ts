export function topicLabel(title: string | null | undefined, id: number): string {
  return title || `話題 #${id}`;
}

export function parseTopicId(param: string | null): number | undefined {
  if (param === null || !/^\d+$/.test(param)) return undefined;
  const id = Number(param);
  return Number.isSafeInteger(id) && id > 0 ? id : undefined;
}
