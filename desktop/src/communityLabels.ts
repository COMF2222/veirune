export function listenerLabel(count: number, english = false): string {
  if (english) return `${count} ${count === 1 ? 'listener' : 'listeners'}`;
  const last = count % 10, pair = count % 100;
  return `${count} ${last === 1 && pair !== 11 ? 'слушатель' : last >= 2 && last <= 4 && (pair < 12 || pair > 14) ? 'слушателя' : 'слушателей'}`;
}
