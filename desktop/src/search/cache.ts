// Session-only cache: bounded retention, TTL and shared in-flight requests.
export class SearchCache<T> {
  private entries = new Map<string, { value: T; expires: number }>();
  private epoch = 0;
  private pending = new Map<string, Promise<T>>();
  constructor(private capacity = 20, private ttlMs = 5 * 60_000, private now = Date.now, private retain: (value: T) => boolean = () => true) {}
  set(key: string, value: T) {
    this.entries.delete(key);
    if (!this.retain(value)) return;
    this.entries.set(key, { value, expires: this.now() + this.ttlMs });
    while (this.entries.size > this.capacity) this.entries.delete(this.entries.keys().next().value!);
  }
  clear() { this.epoch += 1; this.entries.clear(); this.pending.clear(); }
  getOrLoad(key: string, load: () => Promise<T>): Promise<T> {
    const entry = this.entries.get(key);
    if (entry && entry.expires > this.now()) {
      this.entries.delete(key);
      this.entries.set(key, entry);
      return Promise.resolve(entry.value);
    }
    this.entries.delete(key);
    const pending = this.pending.get(key);
    if (pending) return pending;
    const epoch = this.epoch;
    const request = Promise.resolve().then(load).then((value) => {
      if (epoch === this.epoch) this.set(key, value);
      return value;
    }).finally(() => { if (this.pending.get(key) === request) this.pending.delete(key); });
    this.pending.set(key, request);
    return request;
  }
}
export function searchKey(keyword: string, type: string, mode: string) {
  return JSON.stringify([keyword.trim().replace(/\s+/g, " "), type, mode]);
}
